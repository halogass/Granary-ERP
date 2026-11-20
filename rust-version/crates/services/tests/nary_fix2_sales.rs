//! Nary fix batch 2, agent T1 (the Owner's 14-19 Oct review, A1 and A3): sales tools.
//!
//! - A1 a Solution Proposal from the chat: propose_solution_proposal makes a card that shows the deal and the
//!   requirement sheet; applying it writes the sheet and drafts exactly as the automatic flow (the
//!   Acme Packaging case of 14 Oct); update_requirements edits the sheet only;
//!   the cost sheet only for cost viewers; the Owner or sales:create_quotation only.
//! - A3 list_quotations (filters, paging, total) and get_quotation (lines, totals, approval and send
//!   history; costs and margins only for cost viewers); sales:view_quotations only.

mod nary_n6_common;
mod support;

use std::collections::HashSet;

use granary_core::auth::AuthUser;
use granary_db::rows::documents::SalesDocument;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::ai::schema;
use granary_services::ai::tools::catalog::{run_tool, tool_by_name};
use granary_services::nary::{executor, proposals};
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

/// The executor reads the database through one process-wide session pool (`set_session_pool`), so two of
/// these tests running at once would write each other's cards into the wrong database: they take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

/// A user who is not the superadmin (nor the Owner) with exactly these permissions.
async fn staff(db: &mut PgConnection, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("staff")))
    .bind(u("Staff"))
    .execute(&mut *db)
    .await
    .unwrap();
    let mut user = auth_user(db, id).await;
    user.perms = perms.iter().map(|p| p.to_string()).collect::<HashSet<_>>();
    user
}

async fn acme_packaging(db: &mut PgConnection) -> (Cust, Uuid) {
    let cust = customer(db, Some("purchasing@acmepack.test"), false, "en").await;
    sqlx::query("UPDATE organizations SET name = 'Acme Packaging Ltd' WHERE id = $1")
        .bind(cust.org_id)
        .execute(&mut *db)
        .await
        .unwrap();
    let deal = deal_for(db, &cust, "qualified").await;
    sqlx::query("UPDATE deals SET title = 'Acme Packaging: line stop tracking' WHERE id = $1")
        .bind(deal)
        .execute(&mut *db)
        .await
        .unwrap();
    (cust, deal)
}

fn draft_answers(brain: &Brain) {
    brain.set(
        "nary.proposal_draft",
        Answer::json(json!({"title": "Line Stop Tracking", "subtitle": "Acme Packaging", "sections": [
            {"key": "executive_summary", "blocks": [{"type": "text", "md": "We propose line stop tracking on 12 lines."}]},
            {"key": "architecture", "blocks": [{"type": "text", "md": "One server per plant."}]}]})),
    );
    brain.set(
        "nary.cost_estimate",
        Answer::json(json!({"workstreams": [{"name": "Build", "lines": [
            {"role": "Field Technician", "seniority": "mid", "low": 5, "likely": 8, "high": 10}]}],
            "non_labour": [], "category": "custom_ai", "assumptions": []})),
    );
}

fn acme_requirements() -> Value {
    json!({
        "fields": {"problem": "Line stops on the printing lines are not logged and nobody sees the causes",
                   "current_setup": "Paper logs on each line"},
        "standard_items": ["Line tablet", "Support plan"],
        "custom_items": ["Line stop dashboard"],
        "options": [{"name": "Option A", "description": "Server on site"},
                    {"description": "Managed service, monthly"}],
        "assumptions": ["Each line has a network point"],
        "notes": "With a floor diagram",
        "language": "en",
    })
}

// ─── A1 ──────────────────────────────────────────────────────────────────────

/// The 14 Oct case: "make the solution proposal for Acme Packaging now". The card shows
/// the deal and the whole sheet; on the yes the sheet is written and the proposal, its figures and the
/// cost sheet are made; the answer carries the document id and the preview link (and the cost for the Owner).
#[tokio::test]
async fn test_acme_packaging_proposal_from_the_chat() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("t1_acme_packaging").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    rate_card(db).await;
    let (_cust, deal_id) = acme_packaging(db).await;

    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"organization": "Acme Packaging", "requirements": acme_requirements()}),
    )
    .await;
    let card = &out["proposal"];
    assert_eq!(card["action"], "proposal.from_chat", "{out}");
    assert_eq!(card["payload"]["deal_id"], deal_id.to_string());
    let summary = card["summary"].as_str().unwrap();
    for want in [
        "Solution Proposal for Acme Packaging: line stop tracking (Acme Packaging Ltd)",
        "- Problem and the outcome they want: Line stops on the printing lines",
        "- Budget: not known yet (to be confirmed)",
        "Standard items: Line tablet; Support plan",
        "Custom items: Line stop dashboard",
        "Option A: Server on site",
        "Option B: Managed service, monthly",
        "Assumptions: Each line has a network point",
        "Notes: With a floor diagram",
        "Then: Nary drafts the proposal from this sheet, makes its figures and the internal cost sheet",
    ] {
        assert!(summary.contains(want), "missing {want:?} in:\n{summary}");
    }
    assert!(!summary.contains('\u{2014}'), "no em dash: {summary}");
    // nothing written before the yes
    assert!(proposals::sheet(db, deal_id).await.unwrap().is_none());

    draft_answers(&brain);
    let row =
        executor::run_on_instruction(db, &me, "proposal.from_chat", card["payload"].clone(), None)
            .await;
    let row = row.unwrap();
    let result = row.result.clone().unwrap();
    let doc: SalesDocument = sqlx::query_as("SELECT * FROM sales_documents WHERE deal_id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(result["document_id"], doc.id.to_string());
    assert_eq!(result["link"], format!("/sales/proposals/{}", doc.id));
    let label = result["label"].as_str().unwrap();
    assert!(
        label.contains(&doc.id.to_string()) && label.contains("/sales/proposals/"),
        "{label}"
    );
    // the Owner may see costs: the likely cost comes back
    assert!(result["cost_sheet"]["likely"].is_object(), "{result}");
    // the stored card shows the sheet (diff.lines) and the card text renders it in full
    let text = granary_services::nary::cards::render_card(db, &row)
        .await
        .unwrap();
    assert!(
        text.contains("Custom items: Line stop dashboard") && text.contains("Option B"),
        "{text}"
    );
    // the sheet: known fields, scope, ready
    let sheet = proposals::sheet(db, deal_id).await.unwrap().unwrap();
    assert_eq!(sheet.status, "ready");
    assert_eq!(sheet.content["fields"]["current_setup"]["known"], true);
    assert_eq!(sheet.content["fields"]["budget"]["known"], false);
    assert_eq!(
        sheet.content["scope"]["custom_items"],
        json!(["Line stop dashboard"])
    );
    assert_eq!(sheet.content["coverage"], 22);
    // the cost sheet was written
    let costs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nary_cost_sheets WHERE document_id = $1")
            .bind(doc.id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(costs, 1);
    // the drafting prompt carries the chat's scope and notes
    let (_, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.proposal_draft")
        .unwrap();
    let content = call.chat().unwrap().messages[0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        content.contains("Line stop dashboard") && content.contains("With a floor diagram"),
        "{content}"
    );

    // a sales person who may not see costs: the same flow, no cost in the answer
    let sales = staff(db, &["sales:create_quotation", "crm:view_all"]).await;
    let again = proposals::apply_from_chat(
        db,
        Some(&sales),
        // fix-longapply: the deal has a proposal now; a second one only when asked for
        &json!({"deal_id": deal_id.to_string(), "requirements": {"standard_items": ["Line monitoring"]}, "new_proposal": true}),
    )
    .await
    .unwrap();
    assert!(again.get("cost_sheet").is_none(), "{again}");
    assert!(again["document_id"].as_str().is_some());
    let sheet = proposals::sheet(db, deal_id).await.unwrap().unwrap();
    assert_eq!(
        sheet.content["scope"]["standard_items"],
        json!(["Line monitoring"])
    );
    // what the first request wrote stays
    assert_eq!(
        sheet.content["scope"]["custom_items"],
        json!(["Line stop dashboard"])
    );
}

#[tokio::test]
async fn test_only_the_owner_or_sales_may_ask_and_bad_requirements_are_refused() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("t1_proposal_perms").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let (_cust, deal_id) = acme_packaging(db).await;
    // no sales permission: the tool is not offered and not run
    let viewer = staff(db, &["crm:view_all"]).await;
    let out = run_tool(
        db,
        &viewer,
        "propose_solution_proposal",
        json!({"deal_id": deal_id.to_string()}),
    )
    .await;
    assert_eq!(out["error"], "You don't have permission to use this tool.");
    let refused = proposals::apply_from_chat(
        db,
        Some(&viewer),
        &json!({"deal_id": deal_id.to_string(), "requirements": {}}),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.status().as_u16(), 403);
    // an unknown checklist key or bad options are told to the model, nothing is proposed
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"deal_id": deal_id.to_string(), "requirements": {"fields": {"colour": "blue"}}}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("Unknown requirement 'colour'"),
        "{out}"
    );
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"deal_id": deal_id.to_string(), "requirements": {"options": "many"}}),
    )
    .await;
    assert!(out["error"].as_str().unwrap().contains("options"), "{out}");
    // a lead that was never promoted has no deal
    let cust = customer(db, None, true, "en").await;
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"lead_id": cust.lead_id.unwrap().to_string()}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("promote the lead first"),
        "{out}"
    );
    // a promoted lead resolves to its deal
    let promoted = deal_for(db, &cust, "qualified").await;
    sqlx::query("UPDATE leads SET converted_to_deal_id = $1, status = 'promoted' WHERE id = $2")
        .bind(promoted)
        .bind(cust.lead_id.unwrap())
        .execute(&mut *db)
        .await
        .unwrap();
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"lead_id": cust.lead_id.unwrap().to_string()}),
    )
    .await;
    assert_eq!(
        out["proposal"]["payload"]["deal_id"],
        promoted.to_string(),
        "{out}"
    );
}

#[tokio::test]
async fn test_update_requirements_edits_only_the_sheet() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("t1_update_requirements").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let (_cust, deal_id) = acme_packaging(db).await;
    let deal: granary_db::rows::crm::Deal = sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let doc = granary_services::documents::core_::create(
        db,
        granary_services::documents::core_::CreateArgs {
            user_id: Some(me.id),
            title: "Acme proposal".into(),
            deal: Some(&deal),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    proposals::start_discovery(db, Some(&me), &deal_id.to_string(), "custom")
        .await
        .unwrap();
    let out = run_tool(
        db,
        &me,
        "update_requirements",
        json!({"document_id": doc.id.to_string(),
               "requirements": {"fields": {"timeline": "Live by January"}, "standard_items": ["8 lines"]}}),
    )
    .await;
    let card = &out["proposal"];
    assert_eq!(card["action"], "proposal.update_requirements", "{out}");
    let summary = card["summary"].as_str().unwrap();
    assert!(
        summary.contains("- Timeline: Live by January")
            && summary.contains("Standard items: 8 lines")
            && summary.contains("Only the sheet changes"),
        "{summary}"
    );
    let empty = run_tool(
        db,
        &me,
        "update_requirements",
        json!({"document_id": doc.id.to_string(), "requirements": {}}),
    )
    .await;
    assert!(empty["error"].as_str().is_some(), "{empty}");
    let row = executor::run_on_instruction(
        db,
        &me,
        "proposal.update_requirements",
        card["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    let label = row.result.as_ref().unwrap()["label"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(label.contains("updated (11% known)"), "{label}");
    let sheet = proposals::sheet(db, deal_id).await.unwrap().unwrap();
    // the discovery keeps running; only the sheet changed, no new proposal
    assert_eq!(sheet.status, "discovery");
    assert_eq!(
        sheet.content["fields"]["timeline"]["value"],
        "Live by January"
    );
    assert_eq!(sheet.content["reason"], "custom");
    let docs: i64 = sqlx::query_scalar("SELECT count(*) FROM sales_documents WHERE deal_id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(docs, 1);
}

// ─── A3 ──────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn quotation(
    db: &mut PgConnection,
    deal: Uuid,
    cust: &Cust,
    status: &str,
    currency: &str,
    issue: &str,
    owner: Option<Uuid>,
    product: Option<(Uuid, String, String)>,
) -> Uuid {
    let id = quotation_for(db, deal, cust, product, status, 2, 15_000_000).await;
    sqlx::query(
        "UPDATE quotations SET currency = $2, issue_date = $3::date, created_by_id = $4, subject = $5, \
         notes = 'Prices exclude VAT', internal_notes = 'Pytest internal' WHERE id = $1",
    )
    .bind(id)
    .bind(currency)
    .bind(issue)
    .bind(owner)
    .bind(u("Quotation"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_list_quotations_filters_pages_and_checks_permission() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("t1_list_quotations").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let (acme, acme_deal) = acme_packaging(db).await;
    let other = customer(db, None, false, "en").await;
    let other_deal = deal_for(db, &other, "qualified").await;
    let seller = staff(db, &["sales:view_quotations"]).await;
    let g1 = quotation(
        db,
        acme_deal,
        &acme,
        "sent",
        "JPY",
        "2026-10-01",
        Some(me.id),
        None,
    )
    .await;
    let g2 = quotation(
        db,
        acme_deal,
        &acme,
        "draft",
        "USD",
        "2026-10-10",
        Some(seller.id),
        None,
    )
    .await;
    let g3 = quotation(
        db,
        acme_deal,
        &acme,
        "accepted",
        "JPY",
        "2026-10-15",
        Some(me.id),
        None,
    )
    .await;
    let o1 = quotation(
        db,
        other_deal,
        &other,
        "sent",
        "JPY",
        "2026-09-01",
        Some(me.id),
        None,
    )
    .await;
    let deleted = quotation(
        db,
        other_deal,
        &other,
        "draft",
        "JPY",
        "2026-10-02",
        None,
        None,
    )
    .await;
    sqlx::query("UPDATE quotations SET is_deleted = true WHERE id = $1")
        .bind(deleted)
        .execute(&mut *db)
        .await
        .unwrap();

    let ids = |out: &Value| -> Vec<String> {
        out["quotations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["id"].as_str().unwrap().to_string())
            .collect()
    };
    let all = run_tool(db, &seller, "list_quotations", json!({})).await;
    assert_eq!(all["total"], 4, "{all}");
    let acme_only = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"organization": "acme packaging"}),
    )
    .await;
    assert_eq!(acme_only["total"], 3);
    assert!(acme_only["quotations"][0]["organization"]
        .as_str()
        .unwrap()
        .contains("Acme Packaging"));
    let by_org_id = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"organization": other.org_id.to_string()}),
    )
    .await;
    assert_eq!(ids(&by_org_id), vec![o1.to_string()]);
    let sent = run_tool(db, &seller, "list_quotations", json!({"status": "sent"})).await;
    assert_eq!(sent["total"], 2);
    let usd = run_tool(db, &seller, "list_quotations", json!({"currency": "usd"})).await;
    assert_eq!(ids(&usd), vec![g2.to_string()]);
    let october = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"date_from": "2026-10-05", "date_to": "2026-10-31", "deal_id": acme_deal.to_string()}),
    )
    .await;
    let mut got = ids(&october);
    got.sort();
    let mut want = vec![g2.to_string(), g3.to_string()];
    want.sort();
    assert_eq!(got, want);
    let mine = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"owner": seller.name}),
    )
    .await;
    assert_eq!(ids(&mine), vec![g2.to_string()]);
    let contact = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"contact": acme.contact_id.to_string()}),
    )
    .await;
    assert_eq!(contact["total"], 3);
    // paging: the total stays, the page moves, next_offset until the end
    let p1 = run_tool(db, &seller, "list_quotations", json!({"limit": 3})).await;
    assert_eq!((p1["total"].as_i64(), ids(&p1).len()), (Some(4), 3));
    assert_eq!(p1["next_offset"], 3);
    let p2 = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"limit": 3, "offset": 3}),
    )
    .await;
    assert_eq!(ids(&p2).len(), 1);
    assert!(p2.get("next_offset").is_none());
    let mut seen: Vec<String> = ids(&p1).into_iter().chain(ids(&p2)).collect();
    seen.sort();
    let mut every = vec![
        g1.to_string(),
        g2.to_string(),
        g3.to_string(),
        o1.to_string(),
    ];
    every.sort();
    assert_eq!(seen, every);
    // bad input is told, not raised
    let bad = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"date_from": "last week"}),
    )
    .await;
    assert!(
        bad["error"].as_str().unwrap().contains("date_from"),
        "{bad}"
    );
    let nobody = run_tool(
        db,
        &seller,
        "list_quotations",
        json!({"owner": "no such person"}),
    )
    .await;
    assert!(nobody["error"].as_str().is_some());
    // permission
    let outsider = staff(db, &["crm:view_all"]).await;
    let denied = run_tool(db, &outsider, "list_quotations", json!({})).await;
    assert_eq!(
        denied["error"],
        "You don't have permission to use this tool."
    );
    let denied = run_tool(
        db,
        &outsider,
        "get_quotation",
        json!({"quotation_id": g1.to_string()}),
    )
    .await;
    assert_eq!(
        denied["error"],
        "You don't have permission to use this tool."
    );
}

#[tokio::test]
async fn test_get_quotation_shows_lines_history_and_costs_only_to_cost_viewers() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("t1_get_quotation").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let (acme, deal) = acme_packaging(db).await;
    let (pid, sku) = priced_product(db, "Pytest Laptop", 5).await;
    let q = quotation(
        db,
        deal,
        &acme,
        "sent",
        "JPY",
        "2026-10-14",
        Some(me.id),
        Some((pid, sku, "Pytest Laptop".into())),
    )
    .await;
    // an approval with one decision, and the send recorded on the deal
    let req = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO approval_requests (id, entity_type, entity_id, status, document_revision, submitted_by_id, \
         submitted_at, completed_at, snapshot, flags, steps) \
         VALUES ($1, 'quotation', $2, 'approved', 1, $3, now() - interval '2 hours', now() - interval '1 hour', '{}', '{}', '[]')",
    )
    .bind(req)
    .bind(q)
    .bind(me.id)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO approval_actions (id, request_id, step_seq, actor_id, action, comment) \
         VALUES ($1, $2, 1, $3, 'approve', 'Fine at list price')",
    )
    .bind(Uuid::new_v4())
    .bind(req)
    .bind(me.id)
    .execute(&mut *db)
    .await
    .unwrap();
    let row: granary_db::rows::sales::Quotation =
        sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
            .bind(q)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    granary_services::crm::deal_hooks::log_quotation_event(db, &row, "sent", Some(me.id), None)
        .await
        .unwrap();

    let seller = staff(db, &["sales:view_quotations"]).await;
    let out = run_tool(
        db,
        &seller,
        "get_quotation",
        json!({"quotation_id": q.to_string()}),
    )
    .await;
    assert_eq!(out["id"], q.to_string(), "{out}");
    assert_eq!(out["status"], "sent");
    assert_eq!(out["lines"].as_array().unwrap().len(), 1);
    assert_eq!(out["lines"][0]["name"], "Pytest Laptop");
    assert_eq!(out["lines"][0]["quantity"].as_f64(), Some(2.0));
    assert_eq!(out["totals"]["total"].as_f64(), Some(30_000_000.0));
    assert_eq!(out["notes"], "Prices exclude VAT");
    assert_eq!(out["terms"]["payment_terms"]["text"], "50% down payment");
    assert_eq!(out["approval_history"][0]["status"], "approved");
    assert_eq!(
        out["approval_history"][0]["decisions"][0]["comment"],
        "Fine at list price"
    );
    assert_eq!(out["send_history"][0]["kind"], "quotation_sent");
    assert!(out["send_history"][0]["subject"]
        .as_str()
        .unwrap()
        .contains("sent"));
    // no cost, margin or tier anywhere for someone who may not see costs
    let text = out.to_string();
    for k in ["margins", "cost", "margin", "tier_available_pct", "9000000"] {
        assert!(!text.contains(k), "{k} leaked: {text}");
    }
    // a cost viewer sees the margin report
    let finance = staff(db, &["sales:view_quotations", "sales:view_costs"]).await;
    let full = run_tool(
        db,
        &finance,
        "get_quotation",
        json!({"quotation_id": q.to_string()}),
    )
    .await;
    assert!(full["margins"].is_object(), "{full}");
    assert!(full["margins"].get("lines").is_some() || full["margins"].get("error").is_some());
    // by number
    let number: String = sqlx::query_scalar("SELECT number FROM quotations WHERE id = $1")
        .bind(q)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let by_number = run_tool(db, &seller, "get_quotation", json!({"number": number})).await;
    assert_eq!(by_number["id"], q.to_string());
    let missing = run_tool(db, &seller, "get_quotation", json!({"number": "NOPE-1"})).await;
    assert!(missing["error"].as_str().is_some());
}

// ─── The tools themselves ────────────────────────────────────────────────────

#[test]
fn test_the_new_tools_are_listed_and_their_schemas_pass_the_anthropic_rules() {
    use granary_services::ai::mcp_access;
    for name in [
        "propose_solution_proposal",
        "update_requirements",
        "list_quotations",
        "get_quotation",
    ] {
        let t = tool_by_name(name).unwrap_or_else(|| panic!("{name} missing"));
        let sent = schema::for_anthropic(&t.parameters);
        // fix-longapply: tool input schemas are exempt from the free-form object rule (structured outputs only,
        // as nary_fix1_writing checks)
        let bad: Vec<String> = schema::violations(&sent)
            .into_iter()
            .filter(|v| !v.contains("free-form object"))
            .collect();
        assert!(bad.is_empty(), "{name}: {bad:?}");
        assert!(!t.description.contains('\u{2014}'), "{name} has an em dash");
    }
    // the propose tools are change tools (a card; read-only MCP connections don't get them), the readers aren't
    for name in ["propose_solution_proposal", "update_requirements"] {
        assert!(mcp_access::PROPOSAL_TOOLS.contains(&name));
    }
    for name in ["list_quotations", "get_quotation"] {
        assert!(!mcp_access::PROPOSAL_TOOLS.contains(&name));
    }
    for a in ["proposal.from_chat", "proposal.update_requirements"] {
        assert!(executor::CHAT_ACTIONS.contains(&a));
        assert!(granary_services::nary::actions::get(a).is_ok());
    }
}

#[test]
fn test_clean_requirements_and_the_merged_sheet() {
    let checklist = vec![
        json!({"key": "problem", "label": "Problem"}),
        json!({"key": "budget", "label": "Budget"}),
    ];
    let clean = proposals::clean_requirements(
        &json!({"fields": {"problem": "  line stops  ", "budget": ""}, "options": ["On-site server"], "language": " en "}),
        &checklist,
    )
    .unwrap();
    assert_eq!(clean["fields"], json!({"problem": "line stops"}));
    assert_eq!(
        clean["options"],
        json!([{"name": "Option A", "description": "On-site server"}])
    );
    assert_eq!(clean["language"], "en");
    assert!(proposals::clean_requirements(&json!({"language": "id"}), &checklist).is_err());
    assert!(proposals::clean_requirements(&json!({"price": 1}), &checklist).is_err());
    assert!(proposals::clean_requirements(&json!({"language": "fr"}), &checklist).is_err());
    let merged = proposals::merged_sheet(None, &clean, &checklist);
    assert_eq!(merged["coverage"], 50);
    assert_eq!(
        merged["fields"]["budget"],
        json!({"known": false, "value": null})
    );
    let lines = proposals::sheet_lines(&merged, &checklist);
    assert_eq!(lines[0], "Requirement sheet (50% known):");
    assert!(lines.contains(&"Language: English".to_string()));
}
