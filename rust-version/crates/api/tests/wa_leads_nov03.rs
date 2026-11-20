//! 3 Nov 2026 (Owner decisions 3 Nov, Stream B1): your AI client (MCP) and the chat do internal Nary's WhatsApp and lead
//! work by hand while `nary.paused` stays on. Every test runs with Nary paused:
//! - propose_whatsapp_lead: a new number (contact + lead + the promotion card, origin chat) through the chat's
//!   Confirm (POST /nary/actions/run); a returning contact gets a lead and the chat joins it; refusals (already a
//!   lead, unknown chat, held chat, a person with an open lead); Nary's own intake is still stopped by the pause.
//! - propose_held_chat_decision: business through the chat; personal at MCP full power waits for nary_apply.
//! - propose_lead_merge: through the chat and at full power (nary_apply); the same lead twice is refused.
//! - score_lead: the BANT breakdown without saving; save=true stores it (full power), refused at propose level.

mod support;

use granary_services::ai::registry::seam::{Answer, Brain};
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "wa_leads_nov03";
const API: &str = "/api/v1";

async fn auth(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .unwrap()
}

fn number() -> String {
    format!("8180{:08}", Uuid::new_v4().as_u128() % 100_000_000)
}

/// A customer WhatsApp chat with one message from them; `status` active or held.
async fn chat(db: &mut PgConnection, name: &str, status: &str) -> (String, String) {
    let p = number();
    let chat_id = format!("{p}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, \
         profile_fetched_at, attribution_asked, last_inbound_at) \
         VALUES ($1, $2, $3, $4, false, $5, true, now(), false, now())",
    )
    .bind(Uuid::new_v4())
    .bind(&chat_id)
    .bind(format!("+{p}"))
    .bind(name)
    .bind(status)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts, processed_at) \
         VALUES ($1, $2, $3, 'inbound', false, 'Hello, we need 20 laptops for our new office in Osaka', \
         now() - interval '1 hour', now())",
    )
    .bind(Uuid::new_v4())
    .bind(format!("true_{chat_id}_{}", Uuid::new_v4().simple()))
    .bind(&chat_id)
    .execute(&mut *db)
    .await
    .unwrap();
    (chat_id, format!("+{p}"))
}

async fn contact_with(db: &mut PgConnection, phone: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, whatsapp, phone, is_primary, tags, is_deleted, custom) \
         VALUES ($1, 'Diana', $2, $3, $3, false, '{}', false, '{}')",
    )
    .bind(id)
    .bind(u("Returning"))
    .bind(phone)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn lead_for(db: &mut PgConnection, contact: Option<Uuid>, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO leads (id, contact_id, source, title, status, priority, custom, interested_products, is_deleted) \
         VALUES ($1, $2, 'website', $3, 'qualifying', 'medium', '{}'::jsonb, '{}', false)",
    )
    .bind(id)
    .bind(contact)
    .bind(title)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn chat_row(db: &mut PgConnection, chat_id: &str) -> (Option<Uuid>, Option<Uuid>, String) {
    sqlx::query_as("SELECT contact_id, lead_id, status FROM wa_chats WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
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

async fn run(app: &TestApp, token: &str, card: &Value) -> support::Resp {
    app.post(
        token,
        &format!("{API}/nary/actions/run"),
        json!({"action": card["action"], "payload": card["payload"]}),
    )
    .await
}

/// Nary paused, the superadmin is the Owner (Nary's approver).
async fn paused(app: &TestApp, db: &mut PgConnection) -> support::TestUser {
    app.set_setting("nary.paused", "true").await;
    let sa = app.superadmin().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut *db)
    .await
    .unwrap();
    sa
}

async fn still_paused(db: &mut PgConnection) {
    assert!(granary_services::nary::governance::is_paused(db)
        .await
        .unwrap());
}

async fn mcp_token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("nov03"), "scopes": scopes, "password": "pytest-pass"}),
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

// ─── propose_whatsapp_lead ────────────────────────────────────────────────────

#[tokio::test]
async fn test_whatsapp_lead_from_a_new_number_through_the_chat_while_paused() {
    let app = app(TAG);
    let _g = app.serial().await;
    let mut db = app.db().await;
    let sa = paused(app, &mut db).await;
    let owner = auth(&mut db, sa.id).await;
    let name = u("Rachel");
    let (chat_id, phone) = chat(&mut db, &name, "active").await;
    let org = u("Northwind Traders");

    let out = granary_services::nary::wa_leads::_h_propose_whatsapp_lead(
        &mut db,
        &owner,
        json!({"phone": phone, "organization": org, "notes": "Asked for 20 laptops"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "lead.from_whatsapp", "{out}");
    assert_eq!(card["payload"]["chat_id"], chat_id.as_str());
    let text = details(card);
    assert!(
        text.contains("Contact: new") && text.contains("lead.promote") && text.contains(&org),
        "{text}"
    );
    assert!(!text.contains('\u{2014}'), "{text}");

    // the chat's Confirm
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (contact_id, lead_id, _) = chat_row(&mut db, &chat_id).await;
    let (contact_id, lead_id) = (contact_id.unwrap(), lead_id.unwrap());
    let (source, title, status, lead_org, by): (
        String,
        String,
        String,
        Option<Uuid>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT source, title, status, organization_id, created_by_id FROM leads WHERE id = $1",
    )
    .bind(lead_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (source.as_str(), status.as_str()),
        ("whatsapp_direct", "qualifying")
    );
    assert_eq!(title, format!("WhatsApp: {name}"));
    assert_eq!(by, Some(sa.id));
    let org_name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(lead_org.unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(org_name, org);
    let (first, contact_org): (String, Option<Uuid>) =
        sqlx::query_as("SELECT first_name, organization_id FROM contacts WHERE id = $1")
            .bind(contact_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(first, name);
    assert_eq!(contact_org, lead_org);
    let note: String = sqlx::query_scalar(
        "SELECT content FROM activities WHERE lead_id = $1 AND activity_type = 'note'",
    )
    .bind(lead_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(note, "Asked for 20 laptops");
    // the promotion card is raised although Nary is paused (it is the person's, origin chat)
    let origin: String = sqlx::query_scalar(
        "SELECT origin FROM agent_pending_actions WHERE action = 'lead.promote' AND payload ->> 'lead_id' = $1",
    )
    .bind(lead_id.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(origin, "chat");
    let event: Value = sqlx::query_scalar(
        "SELECT payload FROM domain_events WHERE type = 'lead.created' AND entity_id = $1",
    )
    .bind(lead_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(event["origin"], "chat");
    still_paused(&mut db).await;

    // again: the chat already belongs to an open lead (or the deal it was promoted to), named
    let again = granary_services::nary::wa_leads::_h_propose_whatsapp_lead(
        &mut db,
        &owner,
        json!({"chat_id": chat_id}),
    )
    .await
    .unwrap();
    let why = again["error"].as_str().unwrap_or_default();
    assert!(why.contains("already belongs to the open"), "{again}");
    // an unknown chat
    let none = granary_services::nary::wa_leads::_h_propose_whatsapp_lead(
        &mut db,
        &owner,
        json!({"chat_id": "8100000000001@c.us"}),
    )
    .await
    .unwrap();
    assert!(
        none["error"]
            .as_str()
            .unwrap()
            .contains("No such WhatsApp chat"),
        "{none}"
    );
    // a held chat: decided first
    let (held, _) = chat(&mut db, &u("Held"), "held").await;
    let out = granary_services::nary::wa_leads::_h_propose_whatsapp_lead(
        &mut db,
        &owner,
        json!({"chat_id": held}),
    )
    .await
    .unwrap();
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("propose_held_chat_decision"),
        "{out}"
    );

    // Nary's own intake is still stopped by the pause: the lead is made, no promotion card
    let (own, _) = chat(&mut db, &u("Own"), "active").await;
    let mut row: granary_db::rows::nary::WaChat =
        sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
            .bind(&own)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let lead = granary_services::nary::channels::create_intake(&mut db, &mut row, "Hello", None)
        .await
        .unwrap()
        .unwrap();
    let cards: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_pending_actions WHERE action = 'lead.promote' AND payload ->> 'lead_id' = $1",
    )
    .bind(lead.id.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(cards, 0);
    assert_eq!(lead.created_by_id, None);
    app.set_setting("nary.paused", "false").await;
}

#[tokio::test]
async fn test_whatsapp_lead_for_a_returning_contact_and_a_person_with_an_open_lead() {
    let app = app(TAG);
    let _g = app.serial().await;
    let mut db = app.db().await;
    let sa = paused(app, &mut db).await;
    let owner = auth(&mut db, sa.id).await;

    // the number belongs to a contact with no open lead: a lead for them, the chat joins it
    let (chat_id, phone) = chat(&mut db, "Diana WA", "active").await;
    let known = contact_with(&mut db, &phone).await;
    let out = granary_services::nary::wa_leads::_h_propose_whatsapp_lead(
        &mut db,
        &owner,
        json!({"chat_id": chat_id, "name": "Someone Else"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    let text = details(card);
    assert!(
        text.contains("returning contact") && text.contains("name given is not used"),
        "{text}"
    );
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (contact_id, lead_id, _) = chat_row(&mut db, &chat_id).await;
    assert_eq!(contact_id, Some(known));
    let lead_contact: Option<Uuid> =
        sqlx::query_scalar("SELECT contact_id FROM leads WHERE id = $1")
            .bind(lead_id.unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(lead_contact, Some(known));
    let linked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM wa_messages WHERE chat_id = $1 AND lead_id = $2")
            .bind(&chat_id)
            .bind(lead_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(linked, 1);

    // a person who already has an open lead: refused, the lead named
    let (other, phone2) = chat(&mut db, "John WA", "active").await;
    let john = contact_with(&mut db, &phone2).await;
    let title = u("John office laptops");
    let open = lead_for(&mut db, Some(john), &title).await;
    let out = granary_services::nary::wa_leads::_h_propose_whatsapp_lead(
        &mut db,
        &owner,
        json!({"chat_id": other}),
    )
    .await
    .unwrap();
    let why = out["error"].as_str().unwrap();
    assert!(
        why.contains(&title)
            && why.contains(&open.to_string())
            && why.contains("propose_chat_link"),
        "{out}"
    );
    still_paused(&mut db).await;
    app.set_setting("nary.paused", "false").await;
}

// ─── held chats and lead merge through the chat ───────────────────────────────

#[tokio::test]
async fn test_held_chat_business_and_lead_merge_through_the_chat_while_paused() {
    let app = app(TAG);
    let _g = app.serial().await;
    let mut db = app.db().await;
    let sa = paused(app, &mut db).await;
    let owner = auth(&mut db, sa.id).await;

    let (held, _) = chat(&mut db, &u("Held biz"), "held").await;
    let out = granary_services::nary::wa_leads::_h_propose_held_chat_decision(
        &mut db,
        &owner,
        json!({"chat_id": held, "decision": "business", "reason": "a supplier"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "chat.held_decide", "{out}");
    assert!(details(card).contains("Why: a supplier"));
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(chat_row(&mut db, &held).await.2, "active");
    // not held any more: nothing to decide
    let again = granary_services::nary::wa_leads::_h_propose_held_chat_decision(
        &mut db,
        &owner,
        json!({"chat_id": held, "decision": "personal"}),
    )
    .await
    .unwrap();
    assert!(
        again["error"].as_str().unwrap().contains("is not held"),
        "{again}"
    );
    let bad = granary_services::nary::wa_leads::_h_propose_held_chat_decision(
        &mut db,
        &owner,
        json!({"chat_id": held, "decision": "maybe"}),
    )
    .await
    .unwrap();
    assert!(bad["error"]
        .as_str()
        .unwrap()
        .contains("business or personal"));

    // lead merge: the existing lead.merge action as a card, confirmed in the chat
    let keep = lead_for(&mut db, None, &u("Keep lead")).await;
    let dup = lead_for(&mut db, None, &u("Dup lead")).await;
    let out = granary_services::nary::wa_leads::_h_propose_lead_merge(
        &mut db,
        &owner,
        json!({"keep_lead_id": keep.to_string(), "merge_lead_id": dup.to_string()}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "lead.merge", "{out}");
    assert_eq!(card["payload"]["drop_id"], dup.to_string().as_str());
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (status, reason): (String, Option<String>) =
        sqlx::query_as("SELECT status, disqualification_reason FROM leads WHERE id = $1")
            .bind(dup)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("disqualified", Some("duplicate"))
    );
    // the same lead twice, and a lead already merged away
    let same = granary_services::nary::wa_leads::_h_propose_lead_merge(
        &mut db,
        &owner,
        json!({"keep_lead_id": keep.to_string(), "merge_lead_id": keep.to_string()}),
    )
    .await
    .unwrap();
    assert!(
        same["error"].as_str().unwrap().contains("same lead"),
        "{same}"
    );
    let gone = granary_services::nary::wa_leads::_h_propose_lead_merge(
        &mut db,
        &owner,
        json!({"keep_lead_id": keep.to_string(), "merge_lead_id": dup.to_string()}),
    )
    .await
    .unwrap();
    assert!(
        gone["error"]
            .as_str()
            .unwrap()
            .contains("already disqualified"),
        "{gone}"
    );
    still_paused(&mut db).await;
    app.set_setting("nary.paused", "false").await;
}

// ─── Your AI client (MCP) at full power ────────────────────────────────────────

#[tokio::test]
async fn test_full_power_while_paused_lead_applies_merge_and_personal_wait_for_nary_apply() {
    let app = app(TAG);
    let _g = app.serial().await;
    let mut db = app.db().await;
    let sa = paused(app, &mut db).await;
    let full = mcp_token(app, &sa.token, &["full"]).await;

    let r = app
        .post(
            &full,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
    for t in [
        "propose_whatsapp_lead",
        "propose_held_chat_decision",
        "propose_lead_merge",
        "score_lead",
    ] {
        assert!(r.text().contains(t), "{t}");
    }

    // a lead from a chat: applied at once (nothing to the customer, nothing deleted)
    let (chat_id, _) = chat(&mut db, &u("Full"), "active").await;
    let out = call(
        app,
        &full,
        "propose_whatsapp_lead",
        json!({"chat_id": chat_id}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    let (_, lead_id, _) = chat_row(&mut db, &chat_id).await;
    let lead_id = lead_id.expect("the chat joined its new lead");
    let out = call(
        app,
        &full,
        "propose_whatsapp_lead",
        json!({"chat_id": chat_id}),
    )
    .await;
    assert!(
        out.to_string().contains("already belongs to the open"),
        "{out}"
    );

    // personal deletes what was stored: parked until nary_apply
    let (held, _) = chat(&mut db, &u("Personal"), "held").await;
    let out = call(
        app,
        &full,
        "propose_held_chat_decision",
        json!({"chat_id": held, "decision": "personal"}),
    )
    .await;
    assert_eq!(out["approval_required"], true, "{out}");
    assert_eq!(chat_row(&mut db, &held).await.2, "held");
    let applied = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": out["apply_id"]}),
    )
    .await;
    assert_eq!(applied["applied"], true, "{applied}");
    let left: Option<String> = sqlx::query_scalar("SELECT status FROM wa_chats WHERE chat_id = $1")
        .bind(&held)
        .fetch_optional(&mut db)
        .await
        .unwrap();
    assert_eq!(left.as_deref(), Some("excluded"));
    let msgs: i64 = sqlx::query_scalar("SELECT count(*) FROM wa_messages WHERE chat_id = $1")
        .bind(&held)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(msgs, 0, "a personal chat's messages are deleted");
    // business applies at once
    let (biz, _) = chat(&mut db, &u("Business"), "held").await;
    let out = call(
        app,
        &full,
        "propose_held_chat_decision",
        json!({"chat_id": biz, "decision": "business"}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    assert_eq!(chat_row(&mut db, &biz).await.2, "active");

    // a merge waits for nary_apply
    let dup = lead_for(&mut db, None, &u("Full dup")).await;
    let out = call(
        app,
        &full,
        "propose_lead_merge",
        json!({"keep_lead_id": lead_id.to_string(), "merge_lead_id": dup.to_string()}),
    )
    .await;
    assert_eq!(out["approval_required"], true, "{out}");
    let status: String = sqlx::query_scalar("SELECT status FROM leads WHERE id = $1")
        .bind(dup)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(status, "qualifying");
    let applied = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": out["apply_id"]}),
    )
    .await;
    assert_eq!(applied["applied"], true, "{applied}");
    let status: String = sqlx::query_scalar("SELECT status FROM leads WHERE id = $1")
        .bind(dup)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(status, "disqualified");
    let out = call(
        app,
        &full,
        "propose_lead_merge",
        json!({"keep_lead_id": dup.to_string(), "merge_lead_id": dup.to_string()}),
    )
    .await;
    assert!(out.to_string().contains("same lead"), "{out}");
    still_paused(&mut db).await;
    app.set_setting("nary.paused", "false").await;
}

// ─── score_lead ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_score_lead_breakdown_save_at_full_power_refused_at_propose() {
    let app = app(TAG);
    let _g = app.serial().await;
    let brain = Brain::new();
    brain.set(
        "nary.score",
        Answer::json(
            json!({"budget": 2, "authority": 3, "need": 2, "timeline": 1,
                            "red_flags": [], "green_flags": ["named budget"], "disqualify": false,
                            "rationale": "Budget approved for Q1", "intent": "quotation",
                            "product": "unknown"}),
        ),
    );
    let _seam = brain.install().await;
    let mut db = app.db().await;
    let sa = paused(app, &mut db).await;
    let owner = auth(&mut db, sa.id).await;
    let (chat_id, phone) = chat(&mut db, &u("Score"), "active").await;
    let contact = contact_with(&mut db, &phone).await;
    let lead = lead_for(&mut db, Some(contact), &u("Scored lead")).await;
    sqlx::query("UPDATE wa_chats SET contact_id = $2, lead_id = $3 WHERE chat_id = $1")
        .bind(&chat_id)
        .bind(contact)
        .bind(lead)
        .execute(&mut db)
        .await
        .unwrap();

    // without save: the breakdown, nothing stored
    let out = granary_services::nary::wa_leads::_h_score_lead(
        &mut db,
        &owner,
        json!({"lead_id": lead.to_string()}),
    )
    .await
    .unwrap();
    assert_eq!(out["total"], 8, "{out}");
    assert_eq!(out["class"], "warm");
    assert_eq!(out["bant"]["authority"], 3);
    assert_eq!(out["saved"], false);
    assert!(out["class"].as_str().is_some_and(|c| !c.is_empty()));
    assert!(brain.jobs().iter().any(|j| j == "nary.score"));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM nary_scores WHERE lead_id = $1")
        .bind(lead)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    // a propose-level connection can't save
    let propose = mcp_token(app, &sa.token, &["propose"]).await;
    let out = call(
        app,
        &propose,
        "score_lead",
        json!({"lead_id": lead.to_string(), "save": true}),
    )
    .await;
    assert!(out.to_string().contains("save"), "{out}");
    let out = call(
        app,
        &propose,
        "score_lead",
        json!({"lead_id": lead.to_string(), "save": false}),
    )
    .await;
    assert_eq!(out["saved"], false, "{out}");

    // full power: saved as Nary saves it
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let out = call(
        app,
        &full,
        "score_lead",
        json!({"lead_id": lead.to_string(), "save": true}),
    )
    .await;
    assert_eq!(out["saved"], true, "{out}");
    let (total, chat): (i32, Option<String>) =
        sqlx::query_as("SELECT total, chat_id FROM nary_scores WHERE lead_id = $1")
            .bind(lead)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((total, chat.as_deref()), (8, Some(chat_id.as_str())));
    let ai: Value = sqlx::query_scalar("SELECT ai_score FROM leads WHERE id = $1")
        .bind(lead)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(ai["score"], 8);
    assert_eq!(ai["scored_by"], "nary");

    // an unknown lead
    let out = call(
        app,
        &full,
        "score_lead",
        json!({"lead_id": Uuid::new_v4().to_string()}),
    )
    .await;
    assert!(out.to_string().contains("not found"), "{out}");
    still_paused(&mut db).await;
    app.set_setting("nary.paused", "false").await;
}

// ─── 5 Nov 2026: propose_lead_promote ─────────────────────────────────────────

#[tokio::test]
async fn test_lead_promote_card_through_the_chat_while_paused() {
    let app = app(TAG);
    let _g = app.serial().await;
    let mut db = app.db().await;
    let sa = paused(app, &mut db).await;
    let owner = auth(&mut db, sa.id).await;

    let lead = lead_for(&mut db, None, &u("Promote me")).await;
    let out = granary_services::nary::wa_leads::_h_propose_lead_promote(
        &mut db,
        &owner,
        json!({"lead_id": lead.to_string()}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "lead.promote", "{out}");
    assert_eq!(card["payload"]["lead_id"], lead.to_string().as_str());
    assert!(details(card).contains("After:"), "{out}");
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (status, deal): (Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT status, converted_to_deal_id FROM leads WHERE id = $1")
            .bind(lead)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(status.as_deref(), Some("promoted"));
    let deal = deal.expect("a deal");
    // promoted already: answers with the deal, no card
    let again = granary_services::nary::wa_leads::_h_propose_lead_promote(
        &mut db,
        &owner,
        json!({"lead_id": lead.to_string()}),
    )
    .await
    .unwrap();
    assert_eq!(again["unchanged"], true, "{again}");
    assert_eq!(again["deal_id"], deal.to_string().as_str());
    // a bad id and a disqualified lead are refused
    let bad = granary_services::nary::wa_leads::_h_propose_lead_promote(
        &mut db,
        &owner,
        json!({"lead_id": "nope"}),
    )
    .await
    .unwrap();
    assert!(bad["error"].as_str().unwrap().contains("lead id"), "{bad}");
    let dq = lead_for(&mut db, None, &u("Disq lead")).await;
    sqlx::query("UPDATE leads SET status = 'disqualified' WHERE id = $1")
        .bind(dq)
        .execute(&mut db)
        .await
        .unwrap();
    let no = granary_services::nary::wa_leads::_h_propose_lead_promote(
        &mut db,
        &owner,
        json!({"lead_id": dq.to_string()}),
    )
    .await
    .unwrap();
    assert!(
        no["error"].as_str().unwrap().contains("is disqualified"),
        "{no}"
    );
    still_paused(&mut db).await;
    app.set_setting("nary.paused", "false").await;
}
