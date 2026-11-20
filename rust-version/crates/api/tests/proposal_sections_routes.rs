//! 23 Oct 2026, through the real app: your AI client (MCP) at full power (scopes ["full"] and ["act", "owner"], the
//! superadmin as Nary's approver) writes a Solution Proposal itself with propose_proposal_from_sections. The
//! card applies at once (act level) and answers with the document; a block with a price is refused with the
//! reason and nothing is made. The internal paths are crates/services/tests/proposal_sections.rs.

mod support;

use granary_services::ai::registry::seam::{Answer, Brain};
use serde_json::{json, Value};
use support::{u, TestApp};
use uuid::Uuid;

const TAG: &str = "proposal_sections_routes";
const API: &str = "/api/v1";

fn app() -> &'static TestApp {
    support::app(TAG)
}

async fn deal(app: &TestApp) -> Uuid {
    let mut db = app.db().await;
    let org_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, type, revenue_currency, tags, is_deleted, custom) \
         VALUES ($1, $2, '{prospect}', 'USD', '{}', false, '{}')",
    )
    .bind(org_id)
    .bind(u("Globex Corp"))
    .execute(&mut db)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, organization_id, stage, currency, value, probability, \
         product_interests, product_line_ids, custom, is_deleted) \
         VALUES ($1, $2, $3, 'proposal', 'JPY', 100000000, 20, '{}', '{}', '{}', false)",
    )
    .bind(id)
    .bind(u("Pytest deal"))
    .bind(org_id)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("sections"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

/// tools/call: (the JSON of the text item, isError).
async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, bool) {
    let r = app
        .post(
            h,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": name, "arguments": args}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let res = r.json()["result"].clone();
    let text = res["content"][0]["text"].as_str().unwrap_or("").to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, res.get("isError") == Some(&json!(true)))
}

async fn proposals_of(app: &TestApp, deal: Uuid) -> Vec<(Uuid, String)> {
    let mut db = app.db().await;
    sqlx::query_as(
        "SELECT id, number FROM sales_documents WHERE deal_id = $1 AND is_deleted = false ORDER BY created_at",
    )
    .bind(deal)
    .fetch_all(&mut db)
    .await
    .unwrap()
}

#[tokio::test]
async fn mcp_client_at_full_power_writes_a_proposal_from_sections() {
    let app = app();
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    app.set_setting("nary.apply_wait_seconds", "45").await;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    brain.set(
        "nary.cost_estimate",
        Answer::json(json!({"workstreams": [{"name": "Build", "lines": [
            {"role": "Field Technician", "seniority": "mid", "low": 5, "likely": 8, "high": 10}]}],
            "non_labour": [], "category": "custom_ai", "assumptions": []})),
    );
    for scopes in [vec!["full"], vec!["act", "owner"]] {
        let h = token(app, &sa.token, &scopes).await;
        let d = deal(app).await;

        // a price in a block: refused with the block named, nothing made
        let (out, is_error) = call(
            app,
            &h,
            "propose_proposal_from_sections",
            json!({"deal_id": d.to_string(), "title": "Office Laptop Rollout", "sections": [
                {"key": "executive_summary", "blocks": [{"type": "text", "md": "Only USD 9,000 for everything."}]}]}),
        )
        .await;
        assert!(is_error, "{scopes:?}: {out}");
        let t = out.to_string();
        assert!(
            t.contains("section executive_summary, block 1 (text)") && t.contains("money"),
            "{t}"
        );
        assert!(proposals_of(app, d).await.is_empty());

        // the sections as written: applied at once, the document answered
        let (out, is_error) = call(
            app,
            &h,
            "propose_proposal_from_sections",
            json!({"deal_id": d.to_string(), "title": "Office Laptop Rollout", "subtitle": "Globex Corp",
                   "sections": [
                       {"key": "executive_summary", "blocks": [
                           {"type": "text", "md": "12 laptops and monitors at two sites."}]},
                       {"key": "implementation", "blocks": [
                           {"type": "text", "md": "Order, set up, handover."}]}],
                   "phases": {"time_unit": "week", "tasks": [{"name": "Install", "start": 0, "length": 2}]}}),
        )
        .await;
        assert!(!is_error, "{scopes:?}: {out}");
        assert_eq!(out["applied"], json!(true), "{scopes:?}: {out}");
        let made = proposals_of(app, d).await;
        assert_eq!(made.len(), 1, "{out}");
        let label = out["result"]["label"].as_str().unwrap_or_default();
        assert!(label.contains(&made[0].0.to_string()), "{out}");
        assert!(!brain.jobs().iter().any(|j| j == "nary.proposal_draft"));
    }
}
