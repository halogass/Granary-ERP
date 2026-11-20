//! 5 Nov 2026 (Stream C of the Owner-approved plan): a sent or approved quotation is linked to a deal without a
//! revision (POST /sales/quotations/{id}/link-deal, Nary's quotation.link_deal card, propose_quotation_link_deal
//! over MCP), every revision of it moves with it, the deal's value follows its total by default, the deal's
//! timeline and the audit log say so, and the customer document (lines, total, status) never changes.
//! propose_quotation_update with only a deal on a sent quotation answers with the link card.

mod support;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "quotation_link_deal_nov05";
const API: &str = "/api/v1";

async fn auth(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .unwrap()
}

async fn org(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, country, type, revenue_currency, tags, is_deleted, custom) \
         VALUES ($1, $2, 'Japan', '{}', 'JPY', '{}', false, '{}')",
    )
    .bind(id)
    .bind(u("Globex Link Ltd"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A deal of `org` worth `value` (typed by hand).
async fn deal(db: &mut PgConnection, org: Option<Uuid>, value: Option<i64>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, organization_id, value, currency, custom, is_deleted) \
         VALUES ($1, $2, 'qualifying', $3, $4, 'JPY', '{}', false)",
    )
    .bind(id)
    .bind(u("Globex rollout"))
    .bind(org)
    .bind(value.map(Decimal::from))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A quotation for `org` without a deal (2 x 500,000 typed), then set to `status`.
async fn quotation(
    app: &TestApp,
    token: &str,
    db: &mut PgConnection,
    org: Uuid,
    status: &str,
) -> (String, String) {
    let r = app
        .post(
            token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "subject": u("Globex SDK"), "validity_days": 14,
                   "customer_org_id": org.to_string(),
                   "items": [{"name": "Installation", "quantity": 2, "unit_price": 500000}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let number = r.json()["full_number"].as_str().unwrap().to_string();
    set_status(db, &id, status).await;
    (id, number)
}

async fn set_status(db: &mut PgConnection, id: &str, status: &str) {
    sqlx::query("UPDATE quotations SET status = $2 WHERE id = $1::uuid")
        .bind(id)
        .bind(status)
        .execute(&mut *db)
        .await
        .unwrap();
}

/// (deal_id, status, total, item count) of a quotation.
async fn facts(db: &mut PgConnection, id: &str) -> (Option<Uuid>, String, Decimal, i64) {
    sqlx::query_as(
        "SELECT q.deal_id, q.status, q.total, (SELECT count(*) FROM quotation_items i WHERE i.quotation_id = q.id) \
         FROM quotations q WHERE q.id = $1::uuid",
    )
    .bind(id)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

async fn deal_value(db: &mut PgConnection, id: Uuid) -> Option<Decimal> {
    sqlx::query_scalar("SELECT value FROM deals WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn activities(
    db: &mut PgConnection,
    deal: Uuid,
    kind: &str,
) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT subject, content FROM activities WHERE deal_id = $1 AND activity_type = $2 ORDER BY created_at",
    )
    .bind(deal)
    .bind(kind)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

async fn link(app: &TestApp, token: &str, qid: &str, body: Value) -> support::Resp {
    app.post(
        token,
        &format!("{API}/sales/quotations/{qid}/link-deal"),
        body,
    )
    .await
}

async fn mcp_token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("nov05link"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> Value {
    let r = app
        .post(
            h,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": name, "arguments": args}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.json()["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }))
}

fn details(card: &Value) -> String {
    card["details"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| d.as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

// ─── the route ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_link_a_sent_quotation_sets_the_deal_value_logs_it_and_keeps_the_document() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let o = org(&mut db).await;
    let d = deal(&mut db, Some(o), Some(750_000)).await;
    let (qid, number) = quotation(app, &sa.token, &mut db, o, "sent").await;
    let (_, status0, total0, items0) = facts(&mut db, &qid).await;

    let r = link(app, &sa.token, &qid, json!({"deal_id": d.to_string()})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["id"], qid.as_str(), "the quotation comes back");
    assert_eq!(r.json()["deal_id"], d.to_string().as_str());
    let (deal_id, status, total, items) = facts(&mut db, &qid).await;
    assert_eq!(deal_id, Some(d));
    assert_eq!(
        (status, total, items),
        (status0, total0, items0),
        "the document is unchanged"
    );
    assert_eq!(deal_value(&mut db, d).await, Some(total0));
    let acts = activities(&mut db, d, "quotation_linked").await;
    assert_eq!(acts.len(), 1);
    assert!(acts[0].0.contains(&number), "{acts:?}");
    let content = acts[0].1.clone().unwrap_or_default();
    assert!(
        content.contains("750,000") && content.contains('\u{2192}'),
        "value old -> new: {content}"
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE entity_type = 'quotation' AND entity_id = $1 \
         AND action = 'quotation_deal_linked' AND origin = 'ui'",
    )
    .bind(&qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    // no approval request was made
    let reqs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM approval_requests WHERE entity_type = 'quotation' AND entity_id = $1::uuid",
    )
    .bind(&qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(reqs, 0);

    // the same deal again: nothing changes, nothing is logged
    let r = link(app, &sa.token, &qid, json!({"deal_id": d.to_string()})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(activities(&mut db, d, "quotation_linked").await.len(), 1);

    // moved to another deal of the customer, keeping that deal's value: both timelines say so
    let d2 = deal(&mut db, Some(o), Some(123_000)).await;
    let r = link(
        app,
        &sa.token,
        &qid,
        json!({"deal_id": d2.to_string(), "set_deal_value": false}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(facts(&mut db, &qid).await.0, Some(d2));
    assert_eq!(deal_value(&mut db, d2).await, Some(Decimal::from(123_000)));
    assert_eq!(activities(&mut db, d2, "quotation_linked").await.len(), 1);
    assert_eq!(activities(&mut db, d, "quotation_unlinked").await.len(), 1);
}

#[tokio::test]
async fn test_every_revision_moves_and_the_value_is_the_current_revision() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let o = org(&mut db).await;
    let d = deal(&mut db, Some(o), None).await;
    let (qid, _) = quotation(app, &sa.token, &mut db, o, "sent").await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/revise"),
            json!({"reason": "New validity"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let rev = r.json()["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE quotations SET total = 2500000 WHERE id = $1::uuid")
        .bind(&rev)
        .execute(&mut db)
        .await
        .unwrap();
    set_status(&mut db, &rev, "sent").await;

    // the superseded one is refused
    let r = link(app, &sa.token, &qid, json!({"deal_id": d.to_string()})).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("superseded"), "{}", r.text());

    let r = link(app, &sa.token, &rev, json!({"deal_id": d.to_string()})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        facts(&mut db, &qid).await.0,
        Some(d),
        "the old revision too"
    );
    assert_eq!(facts(&mut db, &rev).await.0, Some(d));
    assert_eq!(deal_value(&mut db, d).await, Some(Decimal::from(2_500_000)));
}

#[tokio::test]
async fn test_cancelled_other_customer_and_missing_deal_are_refused() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let o = org(&mut db).await;
    let other = org(&mut db).await;
    let d = deal(&mut db, Some(o), Some(1)).await;
    let (cancelled, _) = quotation(app, &sa.token, &mut db, o, "cancelled").await;
    let r = link(
        app,
        &sa.token,
        &cancelled,
        json!({"deal_id": d.to_string()}),
    )
    .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("cancelled"), "{}", r.text());

    let foreign = deal(&mut db, Some(other), Some(9)).await;
    let (qid, _) = quotation(app, &sa.token, &mut db, o, "accepted").await;
    let r = link(
        app,
        &sa.token,
        &qid,
        json!({"deal_id": foreign.to_string()}),
    )
    .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("belongs to"), "{}", r.text());
    assert_eq!(facts(&mut db, &qid).await.0, None);
    assert_eq!(deal_value(&mut db, foreign).await, Some(Decimal::from(9)));

    let r = link(
        app,
        &sa.token,
        &qid,
        json!({"deal_id": Uuid::new_v4().to_string()}),
    )
    .await;
    assert_eq!(r.status, 404, "{}", r.text());

    // an accepted quotation links (its deal follows its offering line) and a deal without organization is fine
    let loose = deal(&mut db, None, None).await;
    let r = link(app, &sa.token, &qid, json!({"deal_id": loose.to_string()})).await;
    assert_eq!(r.status, 200, "{}", r.text());

    // without the permission
    let viewer = app.user_with(&["sales:view_quotations"]).await;
    let r = link(app, &viewer.token, &qid, json!({"deal_id": d.to_string()})).await;
    assert_eq!(r.status, 403, "{}", r.text());
}

// ─── the card, your AI client (MCP), propose_quotation_update ───────────────────────

#[tokio::test]
async fn test_the_card_applies_through_the_action_and_over_mcp() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let owner = auth(&mut db, sa.id).await;
    let o = org(&mut db).await;
    let d = deal(&mut db, Some(o), Some(100)).await;
    let (qid, number) = quotation(app, &sa.token, &mut db, o, "sent").await;
    let total = facts(&mut db, &qid).await.2;
    let total_f: f64 = total.normalize().to_string().parse().unwrap();

    let out = granary_services::nary::quotation_links::_h_propose_quotation_link_deal(
        &mut db,
        &owner,
        json!({"quotation": number, "deal_id": d.to_string()}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "quotation.link_deal", "{out}");
    assert_eq!(card["quotation_number"], number.as_str());
    assert_eq!(card["status"], "sent");
    assert_eq!(card["value_before"].as_f64(), Some(100.0));
    assert_eq!(card["value_after"].as_f64(), Some(total_f));
    let text = details(card);
    assert!(text.contains("Deal: none \u{2192}"), "{text}");
    assert!(!text.contains('\u{2014}'), "{text}");
    let r = app
        .post(
            &sa.token,
            &format!("{API}/nary/actions/run"),
            json!({"action": card["action"], "payload": card["payload"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(facts(&mut db, &qid).await.0, Some(d));
    assert_eq!(deal_value(&mut db, d).await, Some(total));
    let origin: String = sqlx::query_scalar(
        "SELECT origin FROM audit_logs WHERE entity_id = $1 AND action = 'quotation_deal_linked'",
    )
    .bind(&qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(origin, "nary");

    // your AI client (MCP) at full power: applied at once (not money, not a delete)
    let d2 = deal(&mut db, Some(o), None).await;
    let (qid2, number2) = quotation(app, &sa.token, &mut db, o, "sent").await;
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let out = call(
        app,
        &full,
        "propose_quotation_link_deal",
        json!({"quotation": number2, "deal_id": d2.to_string(), "set_deal_value": false}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    assert_eq!(facts(&mut db, &qid2).await.0, Some(d2));
    assert_eq!(deal_value(&mut db, d2).await, None, "set_deal_value false");

    // wiring
    use granary_services::ai::mcp_access;
    assert!(mcp_access::PROPOSAL_TOOLS.contains(&"propose_quotation_link_deal"));
    assert!(!mcp_access::GATED_ACTIONS.contains(&"quotation.link_deal"));
    assert!(granary_services::nary::executor::CHAT_ACTIONS.contains(&"quotation.link_deal"));
    let level = granary_services::nary::autonomy::level_for(&mut db, "quotation.link_deal")
        .await
        .unwrap();
    assert_eq!(level, 1, "Ask");
}

#[tokio::test]
async fn test_quotation_update_with_only_a_deal_on_a_sent_quotation_gives_the_link_card() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let owner = auth(&mut db, sa.id).await;
    let o = org(&mut db).await;
    let d = deal(&mut db, Some(o), None).await;
    let (qid, _) = quotation(app, &sa.token, &mut db, o, "sent").await;

    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": qid, "deal_id": d.to_string(), "items_json": "[]"}),
    )
    .await
    .unwrap();
    assert_eq!(out["proposal"]["action"], "quotation.link_deal", "{out}");
    assert!(
        out["note"]
            .as_str()
            .unwrap_or("")
            .contains("without a revision"),
        "{out}"
    );

    // any other change on a sent quotation is still refused, pointing at the link tool for the deal
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": qid, "deal_id": d.to_string(), "subject": "New subject"}),
    )
    .await
    .unwrap();
    let e = out["error"].as_str().unwrap_or("");
    assert!(
        e.contains("not a draft") && e.contains("propose_quotation_link_deal"),
        "{out}"
    );
    assert_eq!(facts(&mut db, &qid).await.0, None);
}
