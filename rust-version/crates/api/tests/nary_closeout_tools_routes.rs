//! 3 Nov 2026 (Stream B2, Owner decisions 3 Nov): your AI client (MCP) does internal Nary's deal close-out and meeting work
//! by hand while `nary.paused` stays true. Every test runs with Nary paused.
//!
//! - MCP full power: propose_contract_from_quotation applies at once and the down-payment proforma card is raised
//!   although Nary is paused (Nary's own propose_contract is still refused); propose_downpayment_proforma is money
//!   (nary_apply); propose_deal_won refuses with the failing check, then moves the deal once the contract is signed
//!   and the down payment paid; propose_kickoff compiles the handoff package (model call) and starts the project.
//! - Chat apply path (the tool, then the chat's Confirm: POST /nary/actions/run): the same contract card; Won past
//!   the checks only for the Owner with a reason, and over MCP only on the Owner's full-power connection (nary_apply).
//! - find_meeting_slots: read only, the preference, the limits; transcribe_meeting_file reads a meeting's notes
//!   file now (results on the deal, the stage/value card and the summary email card raised while paused).

#[path = "../../services/tests/nary_n10_common/mod.rs"]
mod nary_n10_common;
mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::rhythm;
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_n10_common::{customer, deal_for, Cust};
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{u, TestApp};
use uuid::Uuid;

const TAG: &str = "nary_closeout_tools";
const API: &str = "/api/v1";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl ObjectStore for MemStore {
    fn bucket_exists<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<bool, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(true) })
    }
    fn make_bucket<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(()) })
    }
    fn put_object<'a, 'b, 'd, 'e, 'f, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
        data: &'e [u8],
        _content_type: &'f str,
    ) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        'e: 'c,
        'f: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.blobs
                .lock()
                .unwrap()
                .insert(key.to_string(), data.to_vec());
            Ok(())
        })
    }
    fn get_object<'a, 'b, 'd, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
    ) -> Fut<'c, Result<Vec<u8>, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.blobs
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or(StoreError {
                    kind: "S3Error".into(),
                    message: key.into(),
                })
        })
    }
}

fn app() -> &'static TestApp {
    support::app(TAG)
}

fn brain() -> Brain {
    let b = Brain::p2();
    b.set(
        "nary.handoff",
        Answer::json(json!({
            "summary": "12 laptops, docking and setup, on-site.", "scope_sold": ["laptop setup"],
            "scope_not_sold": ["extra monitors (asked, not bought)"],
            "success_criteria": [{"quote": "Ready on day one", "source": "WhatsApp"}],
            "commitments": ["install within 3 weeks"], "sensitivities": [], "red_flags": ["no IT staff on site"],
            "contacts": ["Robert"], "open_items": []})),
    );
    b.set(
        "nary.transcript_extract",
        Answer::json(json!({
            "summary": "Globex wants 8 laptops for the new office; budget agreed.",
            "bant": {"budget": 3, "authority": 3, "need": 3, "timeline": 2},
            "requirements": ["disk encryption"],
            "commitments": [{"who": "us", "what": "Send the proposal", "due": "2026-11-10"}],
            "objections": [], "next_steps": ["proposal"], "language": "en",
            "deal_update": {"stage": "proposal", "value": 180000000, "currency": "JPY",
                            "reason": "Budget agreed, proposal requested."}})),
    );
    b.set(
        "nary.summary_email",
        Answer::json(json!({"subject": "Meeting summary", "body": "Thank you for your time..."})),
    );
    b
}

/// The superadmin made the Owner (Nary's approver), Nary paused, working hours.
async fn world(app: &TestApp) -> support::TestUser {
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    app.set_setting("nary.paused", "true").await;
    rhythm::set_working_time_override(Some(true));
    sa
}

/// A sent quotation of 30,000,000 JPY, 50% down payment on order and 50% on delivery.
async fn quotation(db: &mut PgConnection, deal_id: Uuid, cust: &Cust, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    let terms = json!({"text": "50% down payment", "schedule": [
        {"label": "Down payment", "percentage": 50.0, "due_trigger": "on_order"},
        {"label": "On delivery", "percentage": 50.0, "due_trigger": "on_milestone"}]});
    let today = granary_core::clock::today_in(granary_core::clock::parse_tz("Asia/Tokyo").unwrap());
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, deal_id, customer_contact_id, customer_org_id, \
         issue_date, valid_until, subtotal, total, payment_terms, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', $3, $4, $5, $6, $7, $8, 30000000, 30000000, $9, false)",
    )
    .bind(id)
    .bind(u("PYT-Q"))
    .bind(status)
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(cust.org_id)
    .bind(today)
    .bind(today + chrono::Duration::days(30))
    .bind(terms)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn stage(db: &mut PgConnection, deal_id: Uuid) -> String {
    sqlx::query_scalar("SELECT stage FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// The cards of an action on this payload key: (status, origin).
async fn cards(
    db: &mut PgConnection,
    action: &str,
    key: &str,
    value: &str,
) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT status, origin FROM agent_pending_actions WHERE action = $1 AND (payload ->> $2) = $3 \
         ORDER BY created_at",
    )
    .bind(action)
    .bind(key)
    .bind(value)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

async fn mcp_token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("b2"), "scopes": scopes, "password": "pytest-pass"}),
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

/// A tool as Nary's chat runs it (on the server's runtime, where Nary's session pool lives).
async fn tool(app: &'static TestApp, user_id: Uuid, name: &'static str, args: Value) -> Value {
    app.on_server(async move {
        let mut db = app.db().await;
        let user = granary_services::nary::roles::load_actor(&mut db, user_id)
            .await
            .unwrap()
            .unwrap();
        granary_services::ai::tools::catalog::run_tool(&mut db, &user, name, args).await
    })
    .await
}

/// The chat's Confirm on a card.
async fn confirm(app: &TestApp, token: &str, out: &Value) -> support::Resp {
    let card = &out["proposal"];
    assert_eq!(card["kind"], "nary_action", "{out}");
    app.post(
        token,
        &format!("{API}/nary/actions/run"),
        json!({"action": card["action"], "payload": card["payload"]}),
    )
    .await
}

fn said(out: &Value) -> String {
    out.to_string()
}

// ─── Your AI client (MCP) at full power ──────────────────────────────────────

#[tokio::test]
async fn test_full_power_closes_a_deal_while_nary_is_paused() {
    let app = app();
    let _g = app.serial().await;
    let b = brain();
    let _seam = b.install().await;
    let sa = world(app).await;
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let mut db = app.db().await;
    let cust = customer(&mut db, None, true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let qid = quotation(&mut db, deal_id, &cust, "sent").await;

    // listed at full power
    let r = app
        .post(
            &full,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
    for t in [
        "propose_contract_from_quotation",
        "propose_downpayment_proforma",
        "propose_deal_won",
        "propose_kickoff",
        "find_meeting_slots",
        "transcribe_meeting_file",
    ] {
        assert!(r.text().contains(t), "{t}");
    }

    // Nary's own path is still paused: its contract card is not raised
    let q: granary_db::rows::sales::Quotation =
        sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
            .bind(qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let own = app
        .on_server(async move {
            let mut db = self::app().db().await;
            granary_services::nary::deals::propose_contract(&mut db, &q, None, "accepted")
                .await
                .unwrap()
        })
        .await;
    assert_eq!(own, None);
    assert!(cards(
        &mut db,
        "contract.create_draft",
        "quotation_id",
        &qid.to_string()
    )
    .await
    .is_empty());

    // the down payment before any contract: refused with the way forward
    let out = call(
        app,
        &full,
        "propose_downpayment_proforma",
        json!({"quotation_id": qid}),
    )
    .await;
    assert!(said(&out).contains("no payment schedule yet"), "{out}");

    // the contract applies at once (act) and the down-payment card follows, although Nary is paused
    let out = call(
        app,
        &full,
        "propose_contract_from_quotation",
        json!({"quotation_id": qid}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    let (cstatus,): (Option<String>,) =
        sqlx::query_as("SELECT status FROM contracts WHERE quotation_id = $1")
            .bind(qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(cstatus.as_deref(), Some("draft"));
    let qstatus: Option<String> = sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1")
        .bind(qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(qstatus.as_deref(), Some("accepted"));
    assert_eq!(stage(&mut db, deal_id).await, "contract");
    let dp: (Uuid,) =
        sqlx::query_as("SELECT id FROM payment_schedules WHERE quotation_id = $1 AND sequence = 1")
            .bind(qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let dp_cards = cards(&mut db, "proforma.raise", "schedule_id", &dp.0.to_string()).await;
    assert_eq!(dp_cards, vec![("pending".to_string(), "nary".to_string())]);
    // twice: refused
    let out = call(
        app,
        &full,
        "propose_contract_from_quotation",
        json!({"quotation_id": qid}),
    )
    .await;
    assert!(said(&out).contains("already has contract"), "{out}");

    // the proforma card already waits; once it is gone, the tool makes it: money waits for nary_apply
    let out = call(
        app,
        &full,
        "propose_downpayment_proforma",
        json!({"deal_id": deal_id}),
    )
    .await;
    assert!(said(&out).contains("already waits"), "{out}");
    sqlx::query(
        "UPDATE agent_pending_actions SET status = 'rejected' WHERE action = 'proforma.raise' \
                 AND (payload ->> 'schedule_id') = $1",
    )
    .bind(dp.0.to_string())
    .execute(&mut db)
    .await
    .unwrap();
    let out = call(
        app,
        &full,
        "propose_downpayment_proforma",
        json!({"deal_id": deal_id}),
    )
    .await;
    assert_eq!(out["approval_required"], true, "{out}");
    assert!(
        out["final_version"]
            .as_str()
            .unwrap()
            .contains("Down payment"),
        "{out}"
    );
    let none: Option<Uuid> =
        sqlx::query_scalar("SELECT proforma_invoice_id FROM payment_schedules WHERE id = $1")
            .bind(dp.0)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(none, None);
    let applied = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": out["apply_id"]}),
    )
    .await;
    assert_eq!(applied["applied"], true, "{applied}");
    let inv: Option<Uuid> =
        sqlx::query_scalar("SELECT proforma_invoice_id FROM payment_schedules WHERE id = $1")
            .bind(dp.0)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let inv = inv.expect("the proforma is raised");
    let kind: Option<String> =
        sqlx::query_scalar("SELECT invoice_type FROM invoices WHERE id = $1")
            .bind(inv)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(kind.as_deref(), Some("proforma"));

    // Won: refused while the contract is unsigned, with the reason
    let out = call(app, &full, "propose_deal_won", json!({"deal_id": deal_id})).await;
    assert!(said(&out).contains("No signed contract"), "{out}");
    assert_eq!(stage(&mut db, deal_id).await, "contract");
    sqlx::query("UPDATE contracts SET status = 'signed' WHERE quotation_id = $1")
        .bind(qid)
        .execute(&mut db)
        .await
        .unwrap();
    let out = call(app, &full, "propose_deal_won", json!({"deal_id": deal_id})).await;
    assert!(said(&out).contains("down payment is not paid"), "{out}");
    sqlx::query("UPDATE invoices SET amount_paid = 15000000 WHERE id = $1")
        .bind(inv)
        .execute(&mut db)
        .await
        .unwrap();
    let out = call(app, &full, "propose_deal_won", json!({"deal_id": deal_id})).await;
    assert_eq!(out["applied"], true, "{out}");
    assert_eq!(stage(&mut db, deal_id).await, "won");
    let out = call(app, &full, "propose_deal_won", json!({"deal_id": deal_id})).await;
    assert!(said(&out).contains("already won"), "{out}");

    // kickoff: the handoff package is compiled on request, then the project starts
    let out = call(app, &full, "propose_kickoff", json!({"deal_id": deal_id})).await;
    assert_eq!(out["applied"], true, "{out}");
    let (number, objectives): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT project_number, objectives FROM projects WHERE deal_id = $1 AND is_deleted = false",
    )
    .bind(deal_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(number.is_some());
    assert!(objectives.unwrap_or_default().contains("Ready on day one"));
    let tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_tasks t JOIN projects p ON p.id = t.project_id WHERE p.deal_id = $1",
    )
    .bind(deal_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(tasks, 5);
    assert_eq!(stage(&mut db, deal_id).await, "handover");
    let out = call(app, &full, "propose_kickoff", json!({"deal_id": deal_id})).await;
    assert!(said(&out).contains("already exists"), "{out}");
    assert!(b.calls().iter().any(|(job, _)| job == "nary.handoff"));
}

// ─── The chat apply path ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_the_chat_cards_and_won_past_the_checks_only_for_the_owner() {
    let app = app();
    let _g = app.serial().await;
    let b = brain();
    let _seam = b.install().await;
    let sa = world(app).await;
    let mut db = app.db().await;
    let cust = customer(&mut db, None, true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let qid = quotation(&mut db, deal_id, &cust, "sent").await;

    // the contract card, confirmed in the chat: the down-payment card follows while Nary is paused
    let out = tool(
        app,
        sa.id,
        "propose_contract_from_quotation",
        json!({"quotation_id": qid}),
    )
    .await;
    assert!(
        out["proposal"]["summary"]
            .as_str()
            .unwrap()
            .contains("marked accepted"),
        "{out}"
    );
    let r = confirm(app, &sa.token, &out).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(stage(&mut db, deal_id).await, "contract");
    let (made, origin): (String, String) = sqlx::query_as(
        "SELECT status, origin FROM agent_pending_actions WHERE action = 'contract.create_draft' \
         AND (payload ->> 'quotation_id') = $1",
    )
    .bind(qid.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((made.as_str(), origin.as_str()), ("applied", "chat"));
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_pending_actions a JOIN payment_schedules s ON s.id::text = a.payload ->> 'schedule_id' \
         WHERE a.action = 'proforma.raise' AND s.quotation_id = $1 AND a.status = 'pending'",
    )
    .bind(qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(waiting, 1);
    let out = tool(
        app,
        sa.id,
        "propose_downpayment_proforma",
        json!({"quotation_id": qid}),
    )
    .await;
    assert!(said(&out).contains("already waits"), "{out}");

    // Won: the checks fail (unsigned); a seller can't force; the Owner must say why
    let seller = app
        .user_with(&["crm:edit_deal", "crm:view_all", "nary:view"])
        .await;
    let out = tool(app, sa.id, "propose_deal_won", json!({"deal_id": deal_id})).await;
    assert!(said(&out).contains("can't move to Won yet"), "{out}");
    let out = tool(
        app,
        seller.id,
        "propose_deal_won",
        json!({"deal_id": deal_id, "force": true,
                                                               "reason": "paid in cash"}),
    )
    .await;
    assert!(said(&out).contains("Only the Owner"), "{out}");
    let out = tool(
        app,
        sa.id,
        "propose_deal_won",
        json!({"deal_id": deal_id, "force": true}),
    )
    .await;
    assert!(said(&out).contains("Say why"), "{out}");
    // a forged chat card is refused at apply for anyone but the Owner
    let r = app
        .post(
            &seller.token,
            &format!("{API}/nary/actions/run"),
            json!({"action": "deal.won", "payload": {"deal_id": deal_id.to_string(), "force": true,
                                                       "reason": "x"}}),
        )
        .await;
    assert!(r.status >= 400, "{}", r.text());
    assert_eq!(stage(&mut db, deal_id).await, "contract");

    // over MCP the Owner's act connection (no full power) can't force either
    let act = mcp_token(app, &sa.token, &["act"]).await;
    let out = call(
        app,
        &act,
        "propose_deal_won",
        json!({"deal_id": deal_id, "force": true,
                                                         "reason": "Signed on paper"}),
    )
    .await;
    assert!(said(&out).contains("Owner's connection"), "{out}");
    assert_eq!(stage(&mut db, deal_id).await, "contract");

    // the Owner forces it with a reason in the chat
    let out = tool(app, sa.id, "propose_deal_won", json!({"deal_id": deal_id, "force": true,
                                                          "reason": "Signed on paper, scan follows"})).await;
    assert!(
        out["proposal"]["summary"]
            .as_str()
            .unwrap()
            .contains("PAST THE CHECKS"),
        "{out}"
    );
    assert!(said(&out).contains("No signed contract"), "{out}");
    let r = confirm(app, &sa.token, &out).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(stage(&mut db, deal_id).await, "won");

    // kickoff in the chat, with a handoff package already there (no model call needed)
    sqlx::query(
        "INSERT INTO nary_insights (id, kind, deal_id, content, status) VALUES ($1, 'handoff', $2, $3, 'draft')",
    )
    .bind(Uuid::new_v4())
    .bind(deal_id)
    .bind(json!({"summary": "From the chat", "success_criteria": [{"quote": "Works at night"}],
                 "scope_not_sold": [], "scope_sold": ["night mode"], "red_flags": []}))
    .execute(&mut db)
    .await
    .unwrap();
    let out = tool(app, sa.id, "propose_kickoff", json!({"deal_id": deal_id})).await;
    assert_eq!(out["handoff_package"]["summary"], "From the chat", "{out}");
    let r = confirm(app, &sa.token, &out).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM projects WHERE deal_id = $1")
        .bind(deal_id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(made, 1);

    // a deal not won yet: no kickoff
    let other = deal_for(&mut db, &cust, "proposal").await;
    let out = tool(app, sa.id, "propose_kickoff", json!({"deal_id": other})).await;
    assert!(said(&out).contains("not won"), "{out}");
}

// ─── Meetings ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_find_meeting_slots_lists_free_times_read_only() {
    let app = app();
    let _g = app.serial().await;
    let sa = world(app).await;
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let mut db = app.db().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM nary_meetings")
        .fetch_one(&mut db)
        .await
        .unwrap();

    let out = call(
        app,
        &full,
        "find_meeting_slots",
        json!({"duration_minutes": 30, "within_days": 14, "prefer": "morning"}),
    )
    .await;
    let slots = out["slots"].as_array().unwrap_or_else(|| panic!("{out}"));
    assert!(!slots.is_empty(), "{out}");
    assert!(slots.len() <= 8, "{out}");
    assert_eq!(out["duration_minutes"], 30);
    // Google is not connected here: said, and only ERP meetings count
    assert!(said(&out).contains("Google Calendar wasn't read"), "{out}");
    if out.get("preference").is_none() {
        for s in slots {
            // "Wed 04 Nov 09:00"
            let hour: u32 = s["company"].as_str().unwrap()[11..13].parse().unwrap();
            assert!(hour < 12, "{s}");
        }
    }
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM nary_meetings")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(before, after, "read only");

    // the same in the chat, with an attendee
    let staff = app.user_with(&["nary:view"]).await;
    let out = tool(
        app,
        sa.id,
        "find_meeting_slots",
        json!({"attendee_user_ids": [staff.id.to_string()], "within_days": 7}),
    )
    .await;
    assert!(out["slots"].is_array(), "{out}");
    assert_eq!(out["attendees"].as_array().map(Vec::len), Some(1), "{out}");

    // refusals: the limit, a bad attendee, a bad preference
    let out = call(
        app,
        &full,
        "find_meeting_slots",
        json!({"within_days": 400}),
    )
    .await;
    assert!(said(&out).contains("nary.meetings.find_max_days"), "{out}");
    let out = call(
        app,
        &full,
        "find_meeting_slots",
        json!({"attendee_user_ids": ["nobody"]}),
    )
    .await;
    assert!(said(&out).contains("not a valid user id"), "{out}");
    let out = tool(
        app,
        sa.id,
        "find_meeting_slots",
        json!({"prefer": "evening"}),
    )
    .await;
    assert!(said(&out).contains("morning, afternoon"), "{out}");
}

async fn meeting(db: &mut PgConnection, cust: &Cust, deal_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_meetings (id, lead_id, deal_id, contact_id, chat_id, channel, status, slots, origin, \
         attendees, title, starts_at, language) VALUES ($1, $2, $3, $4, $5, 'whatsapp', 'booked', '[]', 'chat', '[]', \
         $6, now() - interval '2 hours', 'en')",
    )
    .bind(id)
    .bind(cust.lead_id)
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(&cust.chat_id)
    .bind(title)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_transcribe_meeting_file_reads_the_notes_now_while_paused() {
    let app = app();
    let _g = app.serial().await;
    let b = brain();
    let _seam = b.install().await;
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    let sa = world(app).await;
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let mut db = app.db().await;
    let cust = customer(&mut db, Some("robert.globex@customer.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "discovery").await;
    let mid = meeting(&mut db, &cust, deal_id, "Demo Globex").await;
    let empty = meeting(&mut db, &cust, deal_id, "Intro call").await;

    // no file on the meeting: said so (MCP and chat)
    let out = call(
        app,
        &full,
        "transcribe_meeting_file",
        json!({"meeting_id": empty}),
    )
    .await;
    assert!(said(&out).contains("No recording or notes file"), "{out}");
    let out = tool(
        app,
        sa.id,
        "transcribe_meeting_file",
        json!({"meeting_id": empty}),
    )
    .await;
    assert!(said(&out).contains("No recording or notes file"), "{out}");

    // a notes file waits for the minute tick, which does not run while Nary is paused
    let file_id = app
        .on_server(async move {
            let mut db = self::app().db().await;
            let me = granary_services::nary::roles::load_actor(&mut db, sa.id)
                .await
                .unwrap()
                .unwrap();
            let m: granary_db::rows::nary::NaryMeeting =
                sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
                    .bind(mid)
                    .fetch_one(&mut db)
                    .await
                    .unwrap();
            granary_services::nary::meeting_files::save_upload(
                &mut db,
                &me,
                Some(&m),
                "Notes by Gemini.txt",
                Some("text/plain"),
                b"Notes by Gemini\nRobert: 8 laptops for the new office. Budget agreed. Send the proposal.",
                "upload",
            )
            .await
            .unwrap()
            .id
        })
        .await;

    let out = call(
        app,
        &full,
        "transcribe_meeting_file",
        json!({"meeting_id": mid}),
    )
    .await;
    assert_eq!(out["status"], "done", "{out}");
    assert_eq!(out["id"], file_id.to_string(), "{out}");
    let result = &out["result"];
    assert_eq!(result["commitments"], json!(1), "{out}");
    // the stage/value card and the summary email card are raised although Nary is paused
    let card: Uuid = result["deal_update_card"]
        .as_str()
        .unwrap_or_else(|| panic!("{out}"))
        .parse()
        .unwrap();
    let (action, status): (String, String) =
        sqlx::query_as("SELECT action, status FROM agent_pending_actions WHERE id = $1")
            .bind(card)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        (action.as_str(), status.as_str()),
        ("crm.record_change", "pending")
    );
    let summary: Uuid = result["summary_card"]
        .as_str()
        .unwrap_or_else(|| panic!("{out}"))
        .parse()
        .unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = $1")
            .bind(summary)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(matches!(status.as_str(), "pending" | "shadow"), "{status}");
    let subject: String = sqlx::query_scalar(
        "SELECT subject FROM activities WHERE deal_id = $1 AND activity_type = 'note'",
    )
    .bind(deal_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(subject, "Meeting results: Demo Globex");

    // done is done (MCP and chat)
    let out = call(
        app,
        &full,
        "transcribe_meeting_file",
        json!({"file_id": file_id}),
    )
    .await;
    assert!(said(&out).contains("already read"), "{out}");
    let out = tool(
        app,
        sa.id,
        "transcribe_meeting_file",
        json!({"meeting_id": mid}),
    )
    .await;
    assert!(said(&out).contains("already read"), "{out}");
    // someone without nary:approve can't
    let viewer = app.user_with(&["nary:view"]).await;
    let out = tool(
        app,
        viewer.id,
        "transcribe_meeting_file",
        json!({"meeting_id": mid}),
    )
    .await;
    assert!(out["error"].is_string(), "{out}");
    storage::set_store(None);
}
