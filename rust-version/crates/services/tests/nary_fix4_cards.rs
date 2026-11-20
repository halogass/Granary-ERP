//! Nary fix batch 4, agent D3, item 1: withdraw_cards finds quotation and invoice cards (by kind, customer,
//! subject, deal title), lists waiting cards with id, kind, customer, total and time, withdraws by card_id,
//! says why when nothing was withdrawn; a new quotation card for the same deal or customer replaces the
//! older waiting one in the conversation.
//!
//! Repro (20 Oct 18:00 JST, Initech): `withdraw_cards(match="quotation")` returned [] for a waiting quotation card.

mod support;

use granary_core::auth::AuthUser;
use granary_services::ai::{assistant, card_withdraw};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use tokio::sync::OnceCell;
use uuid::Uuid;

static DB_URL: OnceCell<String> = OnceCell::const_new();

async fn pool() -> PgPool {
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("d3_fix4_cards").await;
            pool.close().await;
            std::env::var("DATABASE_URL").expect("fresh_db sets DATABASE_URL")
        })
        .await;
    PgPool::connect(url).await.expect("connect test db")
}

async fn superadmin(db: &mut PgConnection) -> AuthUser {
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    granary_core::auth::load_active_user(db, id)
        .await
        .unwrap()
        .unwrap()
}

async fn conversation(db: &mut PgConnection, user: &AuthUser, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_conversations (id, user_id, title, created_at, updated_at) VALUES ($1, $2, $3, now(), now())",
    )
    .bind(id)
    .bind(user.id)
    .bind(title)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// An assistant message carrying `cards`, `secs` seconds ago.
async fn cards(db: &mut PgConnection, conv: Uuid, cards: Value, secs: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls, created_at, updated_at) \
         VALUES ($1, $2, 'assistant', 'cards', $3, now() - make_interval(secs => $4), now())",
    )
    .bind(id)
    .bind(conv)
    .bind(json!({"proposals": cards}))
    .bind(secs as f64)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn statuses(db: &mut PgConnection, msg: Uuid) -> Vec<String> {
    let tc: Value = sqlx::query_scalar("SELECT tool_calls FROM ai_messages WHERE id = $1")
        .bind(msg)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    tc["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["status"].as_str().unwrap().to_string())
        .collect()
}

async fn deal(db: &mut PgConnection, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, is_deleted, custom) VALUES ($1, $2, 'proposal', false, '{}')",
    )
    .bind(id)
    .bind(title)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

fn quotation_card(
    pid: &str,
    customer: &str,
    org: Option<&str>,
    deal: Option<Uuid>,
    total: f64,
) -> Value {
    json!({"kind": "quotation", "status": "pending", "proposal_id": pid, "customer": customer,
           "currency": "JPY", "total": total, "warnings": [],
           "payload": {"subject": format!("Quotation for {customer}"), "customer_org_id": org,
                       "customer_contact_id": null, "deal_id": deal.map(|d| d.to_string()),
                       "currency": "JPY", "items": [{"name": "Laptop Server 25", "quantity": 1}]}})
}

#[tokio::test]
async fn test_withdraw_finds_quotation_cards_lists_them_and_says_why() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let db = &mut *db;
    let me = superadmin(db).await;
    let conv = conversation(db, &me, "Initech").await;
    let d = deal(db, "Initech Laptop rollout").await;
    let msg = cards(
        db,
        conv,
        json!([
            quotation_card("q1", "Initech Digital Ltd", None, Some(d), 45_000_000.0),
            {"kind": "customer_message", "channel": "whatsapp", "chat_id": "81901@c.us",
             "summary": "WhatsApp to Adam", "status": "pending", "proposal_id": "m1"},
            {"kind": "invoice", "status": "applied", "proposal_id": "i1", "customer": "Old Ltd",
             "payload": {"items": []}},
        ]),
        60,
    )
    .await;

    // the list: id, kind, customer, deal, total, time
    let listed = assistant::withdraw_cards(db, conv, &json!({}))
        .await
        .unwrap();
    let pending = listed["pending_cards"].as_array().unwrap();
    assert_eq!(pending.len(), 2, "{listed}");
    let q = &pending[0];
    assert_eq!(q["card_id"], "q1");
    assert_eq!(q["kind"], "quotation");
    assert_eq!(q["summary"], "Quotation for Initech Digital Ltd");
    assert_eq!(q["customer"], "Initech Digital Ltd");
    assert_eq!(q["deal"], "Initech Laptop rollout");
    assert_eq!(q["total"], json!(45_000_000.0));
    assert_eq!(q["currency"], "JPY");
    assert!(q["created_at"].as_str().unwrap().ends_with('Z'), "{q}");
    assert_eq!(pending[1]["card_id"], "m1");

    // the repro: "quotation" finds the quotation card (it has no summary or recipient)
    let out = assistant::withdraw_cards(db, conv, &json!({"match": "quotation"}))
        .await
        .unwrap();
    assert_eq!(out["withdrawn"], json!(["quotation"]), "{out}");
    assert_eq!(out["still_pending"].as_array().unwrap().len(), 1);
    assert_eq!(statuses(db, msg).await, ["withdrawn", "pending", "applied"]);

    // nothing withdrawn: already decided, then no match
    let again = assistant::withdraw_cards(db, conv, &json!({"match": "initech quotation"}))
        .await
        .unwrap();
    assert_eq!(again["withdrawn"], json!([]));
    assert_eq!(again["reason"], "already_decided", "{again}");
    assert!(
        again["note"]
            .as_str()
            .unwrap()
            .contains("already withdrawn"),
        "{again}"
    );
    let none = assistant::withdraw_cards(db, conv, &json!({"match": "Zebrafinch"}))
        .await
        .unwrap();
    assert_eq!(none["reason"], "no_match", "{none}");
    assert!(!none["note"].as_str().unwrap().contains('\u{2014}'));

    // by card id
    let by_id = assistant::withdraw_cards(db, conv, &json!({"card_id": "m1"}))
        .await
        .unwrap();
    assert_eq!(by_id["withdrawn"], json!(["WhatsApp to Adam"]), "{by_id}");
    assert_eq!(
        statuses(db, msg).await,
        ["withdrawn", "withdrawn", "applied"]
    );
    let all = assistant::withdraw_cards(db, conv, &json!({"all": true}))
        .await
        .unwrap();
    assert_eq!(all["reason"], "no_pending_cards", "{all}");
}

#[tokio::test]
async fn test_withdraw_finds_by_deal_title_and_points_elsewhere() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let db = &mut *db;
    let me = superadmin(db).await;
    let here = conversation(db, &me, "WhatsApp with Nary").await;
    let there = conversation(db, &me, "Web chat 20 Oct").await;
    let d = deal(db, "Hooli delivery counts").await;
    cards(
        db,
        here,
        json!([quotation_card("h1", "A Ltd", None, None, 1.0)]),
        30,
    )
    .await;
    let other = cards(
        db,
        there,
        json!([quotation_card("o1", "Hooli Consulting", None, Some(d), 2.0)]),
        30,
    )
    .await;

    // a card in another conversation of the same person: not withdrawn, and said where it is
    let out = assistant::withdraw_cards(db, here, &json!({"match": "delivery counts"}))
        .await
        .unwrap();
    assert_eq!(out["withdrawn"], json!([]));
    assert_eq!(out["reason"], "other_conversation", "{out}");
    assert!(
        out["note"].as_str().unwrap().contains("Web chat 20 Oct"),
        "{out}"
    );
    assert_eq!(statuses(db, other).await, ["pending"]);
    let by_id = assistant::withdraw_cards(db, here, &json!({"card_id": "o1"}))
        .await
        .unwrap();
    assert_eq!(by_id["reason"], "other_conversation", "{by_id}");

    // in its own conversation the deal title finds it
    let out = assistant::withdraw_cards(db, there, &json!({"match": "delivery counts"}))
        .await
        .unwrap();
    assert_eq!(out["withdrawn"], json!(["quotation"]), "{out}");

    // a Nary approval card (agent_pending_actions) is pointed at, not touched
    let card = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, customer_facing, payload, diff) \
         VALUES ($1, 'quotation.create_draft', 'nary', 'pending', 1, false, $2, $3)",
    )
    .bind(card)
    .bind(json!({"subject": "Quotation for Umbrella Holdings Ltd", "items": []}))
    .bind(json!({"label": "Quotation for Umbrella Holdings Ltd"}))
    .execute(&mut *db)
    .await
    .unwrap();
    let out = assistant::withdraw_cards(db, here, &json!({"match": "umbrella quotation"}))
        .await
        .unwrap();
    assert_eq!(out["reason"], "approval_card", "{out}");
    assert_eq!(out["approval_cards"][0]["card_id"], card.to_string());
    let status: String =
        sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = $1")
            .bind(card)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(status, "pending");
}

#[tokio::test]
async fn test_new_quotation_card_replaces_the_older_one_for_the_same_deal_or_customer() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let db = &mut *db;
    let me = superadmin(db).await;
    let conv = conversation(db, &me, "Quotes").await;
    let d1 = deal(db, "Initech phase 1").await;
    let d2 = deal(db, "Initech phase 2").await;
    let msg = cards(
        db,
        conv,
        json!([
            quotation_card("a", "Initech Ltd", Some("00000000-0000-0000-0000-0000000000aa"), Some(d1), 1.0),
            quotation_card("b", "Initech Ltd", Some("00000000-0000-0000-0000-0000000000aa"), Some(d2), 2.0),
            quotation_card("c", "Other Ltd", None, None, 3.0),
            {"kind": "customer_message", "channel": "email", "to": "x@y.z", "status": "pending", "proposal_id": "m"},
            {"kind": "nary_action", "action": "quotation.update", "status": "pending", "proposal_id": "u",
             "summary": "Update quotation Q-1", "payload": {"quotation_id": "11111111-1111-1111-1111-111111111111"}},
        ]),
        30,
    )
    .await;

    // same deal: only that one goes (another deal of the same customer stays)
    let new = quotation_card(
        "n1",
        "Initech Ltd",
        Some("00000000-0000-0000-0000-0000000000aa"),
        Some(d1),
        4.0,
    );
    let replaced = card_withdraw::supersede_drafts(db, conv, new.as_object().unwrap())
        .await
        .unwrap();
    assert_eq!(replaced, ["quotation for Initech Ltd"]);
    assert_eq!(
        statuses(db, msg).await,
        ["superseded", "pending", "pending", "pending", "pending"]
    );

    // no deal on either: the same customer (by name here) replaces it
    let new = quotation_card("n2", "other ltd", None, None, 5.0);
    let replaced = card_withdraw::supersede_drafts(db, conv, new.as_object().unwrap())
        .await
        .unwrap();
    assert_eq!(replaced, ["quotation for Other Ltd"]);

    // an invoice card never replaces a quotation; a message card is not looked at
    let inv = json!({"kind": "invoice", "customer": "Initech Ltd", "payload": {"customer_org_id": "00000000-0000-0000-0000-0000000000aa"}});
    assert!(
        card_withdraw::supersede_drafts(db, conv, inv.as_object().unwrap())
            .await
            .unwrap()
            .is_empty()
    );
    let m = json!({"kind": "customer_message", "channel": "email", "to": "x@y.z"});
    assert!(
        card_withdraw::supersede_drafts(db, conv, m.as_object().unwrap())
            .await
            .unwrap()
            .is_empty()
    );

    // a newer update of the same quotation replaces the older update
    let upd = json!({"kind": "nary_action", "action": "quotation.update",
                     "payload": {"quotation_id": "11111111-1111-1111-1111-111111111111"}});
    let replaced = card_withdraw::supersede_drafts(db, conv, upd.as_object().unwrap())
        .await
        .unwrap();
    assert_eq!(replaced, ["Update quotation Q-1"]);
    assert_eq!(
        statuses(db, msg).await,
        [
            "superseded",
            "pending",
            "superseded",
            "pending",
            "superseded"
        ]
    );
}

#[test]
fn the_schemas_pass_the_anthropic_rules_and_the_tools_are_listed() {
    use granary_services::ai::{mcp_access, schema, tools::catalog};
    for name in ["withdraw_cards", "propose_quotation_update"] {
        let t = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        let sent = schema::for_anthropic(&t.parameters);
        assert!(
            schema::violations(&sent).is_empty(),
            "{name}: {:?}",
            schema::violations(&sent)
        );
        assert!(!t.description.contains('\u{2014}'), "{name}");
    }
    let w = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "withdraw_cards")
        .unwrap();
    assert!(w.parameters["properties"]["card_id"].is_object());
    // a card in the chat, a propose-level tool for an MCP connection
    assert!(mcp_access::PROPOSAL_TOOLS.contains(&"propose_quotation_update"));
    let guide = include_str!("../src/ai/erp_guide/nary.md");
    let after = guide
        .split("<!-- added after the Python freeze")
        .nth(1)
        .unwrap();
    assert!(
        after.contains("propose_quotation_update")
            && after.contains("withdraw_cards with no arguments")
    );
    // the action is registered, asks for the quotation permission and can be run from a chat card
    let a = granary_services::nary::actions::get("quotation.update").unwrap();
    assert_eq!(a.permission.as_deref(), Some("sales:create_quotation"));
    assert!(granary_services::nary::executor::CHAT_ACTIONS.contains(&"quotation.update"));
}
