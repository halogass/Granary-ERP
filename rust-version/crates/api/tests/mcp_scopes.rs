//! backend/tests/test_mcp_scopes.py through the real app (the wave-4 stubs `test_mcp_scopes__*` of
//! crates/services/tests/ai_mcp_access.rs), plus the MCP HTTP tests of test_nary_p1_platform.py
//! (`test_mcp_writes_go_through_the_registry`) and test_assistant_access.py
//! (`test_a_portal_user_cannot_mint_an_mcp_token`).

mod support;

use serde_json::{json, Value};
use sqlx::{Connection, PgConnection};
use support::{app, u, TestApp, UserSpec};
use uuid::Uuid;

const TAG: &str = "mcp_scopes";
const API: &str = "/api/v1";

/// `_token(client, *scopes)`: a personal token of `bearer`'s owner at these levels.
async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("scopes"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, h: &str, method: &str, params: Option<Value>) -> Value {
    let mut body = json!({"jsonrpc": "2.0", "id": 1, "method": method});
    if let Some(p) = params {
        body["params"] = p;
    }
    let r = app.post(h, &format!("{API}/mcp"), body).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

/// `_call`: (the tool's JSON answer, or {"text": ...}; isError).
async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        Some(json!({"name": name, "arguments": args})),
    )
    .await["result"]
        .clone();
    let text = res["content"][0]["text"].as_str().unwrap().to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, res.get("isError").is_some_and(|v| v == &json!(true)))
}

async fn names(app: &TestApp, h: &str) -> Vec<String> {
    rpc(app, h, "tools/list", None).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// `_lead()`: Lead(title=_u("MCP lead"), source="email", status="new", custom={}).
async fn lead(app: &TestApp) -> Uuid {
    let mut db = app.db().await;
    sqlx::query_scalar(
        "INSERT INTO leads (id, title, source, status, custom, priority, interested_products, is_deleted) \
         VALUES ($1, $2, 'email', 'new', '{}'::jsonb, 'medium', '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(u("MCP lead"))
    .fetch_one(&mut db)
    .await
    .unwrap()
}

async fn lead_title(app: &TestApp, id: Uuid) -> String {
    let mut db = app.db().await;
    sqlx::query_scalar("SELECT title FROM leads WHERE id = $1")
        .bind(id)
        .fetch_one(&mut db)
        .await
        .unwrap()
}

fn reminder(lead: Uuid) -> Value {
    json!({"entity_type": "lead", "entity_id": lead.to_string(), "title": u("Chase via MCP")})
}

async fn reminders(app: &TestApp, title: &str) -> Vec<Uuid> {
    let mut db = app.db().await;
    sqlx::query_scalar("SELECT owner_user_id FROM crm_reminders WHERE title = $1")
        .bind(title)
        .fetch_all(&mut db)
        .await
        .unwrap()
}

/// `customer()` of nary_p3_support: a contact with a WhatsApp chat the customer started (an open lead).
async fn customer(app: &TestApp) -> String {
    let mut db = app.db().await;
    let phone = format!("8190{:08}", rand_digits());
    let org: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (id, name, type, revenue_currency, tags, is_deleted) \
         VALUES ($1, $2, '{prospect}', 'USD', '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(u("Pytest Ltd"))
    .fetch_one(&mut db)
    .await
    .unwrap();
    let contact: Uuid = sqlx::query_scalar(
        "INSERT INTO contacts (id, first_name, last_name, whatsapp, phone, organization_id, is_primary, tags, \
         is_deleted) VALUES ($1, 'Bob', $2, $3, $3, $4, false, '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(u("Pytest"))
    .bind(format!("+{phone}"))
    .bind(org)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let lead: Uuid = sqlx::query_scalar(
        "INSERT INTO leads (id, contact_id, organization_id, source, title, status, custom, priority, \
         interested_products, is_deleted) VALUES ($1, $2, $3, 'whatsapp_direct', $4, 'qualifying', '{}'::jsonb, \
         'medium', '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(contact)
    .bind(org)
    .bind(u("WhatsApp: Bob"))
    .fetch_one(&mut db)
    .await
    .unwrap();
    let chat_id = format!("{phone}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, status, customer_started, contact_id, triage, \
         language, lead_id, profile_fetched_at, is_my_contact, attribution_asked) \
         VALUES ($1, $2, $3, 'Bob', 'active', true, $4, 'prospect', 'id', $5, now(), false, false)",
    )
    .bind(Uuid::new_v4())
    .bind(&chat_id)
    .bind(format!("+{phone}"))
    .bind(contact)
    .bind(lead)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts, contact_id, \
         processed_at) VALUES ($1, $2, $3, 'inbound', false, 'Hello, I have a question about laptops', \
         now() - interval '1 minute', $4, now())",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(&chat_id)
    .bind(contact)
    .execute(&mut db)
    .await
    .unwrap();
    chat_id
}

fn rand_digits() -> u32 {
    // 8 digits that never start with 0 (090-0xxx is not an assigned mobile range)
    (10_000_000 + Uuid::new_v4().as_u128() % 90_000_000) as u32
}

#[tokio::test]
async fn test_read_lists_no_change_tools_and_refuses_writes() {
    let app = app(TAG);
    let sa = app.superadmin().await.token;
    let h = token(app, &sa, &["read"]).await;
    let names = names(app, &h).await;
    assert!(names.iter().any(|n| n == "search_contacts"));
    for n in [
        "create_crm_record",
        "update_crm_record",
        "propose_followup_reminder",
        "propose_quotation_draft",
        "propose_skill",
        "nary_decide_card",
    ] {
        assert!(!names.iter().any(|x| x == n), "{n}");
    }
    let lead = lead(app).await;
    let (_, err) = call(
        app,
        &h,
        "update_crm_record",
        json!({"object_type": "lead", "record_id": lead.to_string(), "fields": {"title": "Nope"}}),
    )
    .await;
    assert!(err && lead_title(app, lead).await != "Nope");
    let (body, err) = call(app, &h, "propose_followup_reminder", reminder(lead)).await;
    assert!(err && body["text"].as_str().unwrap().contains("access level"));
}

#[tokio::test]
async fn test_propose_makes_a_card_that_applies_as_the_person() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let h = token(app, &sa.token, &["propose"]).await;
    assert!(names(app, &h)
        .await
        .iter()
        .any(|n| n == "propose_followup_reminder"));
    let lead = lead(app).await;
    let args = reminder(lead);
    let title = args["title"].as_str().unwrap().to_string();
    let (body, err) = call(app, &h, "propose_followup_reminder", args).await;
    assert!(
        !err && body["applied"] == json!(false) && body["status"] == "pending",
        "{body}"
    );
    assert!(
        reminders(app, &title).await.is_empty(),
        "nothing changes before approval"
    );
    let card_id = Uuid::parse_str(body["card_id"].as_str().unwrap()).unwrap();
    let mut db = app.db().await;
    let (action, origin, rationale): (String, String, Option<String>) =
        sqlx::query_as("SELECT action, origin, rationale FROM agent_pending_actions WHERE id = $1")
            .bind(card_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((action.as_str(), origin.as_str()), ("mcp.proposal", "mcp"));
    let rationale = rationale.unwrap();
    assert!(rationale.contains("for") && rationale.contains("MCP"));
    let url = app.db_url.clone();
    let sa_id = sa.id;
    app.on_server(async move {
        let mut c = PgConnection::connect(&url).await.unwrap();
        let actor = granary_services::nary::roles::load_actor(&mut c, sa_id)
            .await
            .unwrap()
            .unwrap();
        let mut tx = c.begin().await.unwrap();
        granary_services::nary::executor::decide(&mut tx, card_id, &actor, "apply", None, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await;
    let rows = reminders(app, &title).await;
    assert_eq!(rows, vec![sa.id]);
}

#[tokio::test]
async fn test_act_applies_at_once_but_a_customer_message_stays_a_card() {
    let app = app(TAG);
    let sa = app.superadmin().await.token;
    let h = token(app, &sa, &["act"]).await;
    let lead = lead(app).await;
    let args = reminder(lead);
    let title = args["title"].as_str().unwrap().to_string();
    let (body, err) = call(app, &h, "propose_followup_reminder", args).await;
    assert!(!err && body["applied"] == json!(true), "{body}");
    assert_eq!(reminders(app, &title).await.len(), 1);
    // a customer who wrote first
    let cid = customer(app).await;
    let (body, err) = call(
        app,
        &h,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello, thank you."}),
    )
    .await;
    assert!(
        !err && body["applied"] == json!(false) && body["status"] == "pending",
        "{body}"
    );
}

#[tokio::test]
async fn test_owner_tools_need_full_power_and_money_needs_a_second_call() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await.token;
    let act = token(app, &sa, &["act"]).await;
    assert!(!names(app, &act)
        .await
        .iter()
        .any(|n| n == "nary_decide_card"));
    let owner = token(app, &sa, &["act", "owner"]).await;
    let owner_names = names(app, &owner).await;
    for n in ["nary_open_topics", "nary_decide_card", "nary_pause"] {
        assert!(owner_names.iter().any(|x| x == n), "{n}");
    }

    let lead = lead(app).await;
    let propose = token(app, &sa, &["propose"]).await;
    let (body, _) = call(
        app,
        &propose,
        "update_crm_record",
        json!({"object_type": "lead", "record_id": lead.to_string(), "fields": {"title": "Approved over MCP"}}),
    )
    .await;
    let args = json!({"card_id": body["card_id"], "decision": "apply"});
    // Which decisions touch money is a setting; this one is on it for the test.
    let mut db = app.db().await;
    let old: String = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'nary.owner.confirm_twice_actions'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    let mut list: Vec<Value> = serde_json::from_str(&old).unwrap();
    list.push(json!("mcp.crm_write"));
    app.set_setting(
        "nary.owner.confirm_twice_actions",
        &serde_json::to_string(&list).unwrap(),
    )
    .await;
    let (first, err) = call(app, &owner, "nary_decide_card", args.clone()).await;
    assert!(
        !err && first["approval_required"] == json!(true) && first["applied"] == json!(false),
        "{first}"
    );
    assert_ne!(lead_title(app, lead).await, "Approved over MCP");
    let (wrong, _) = call(
        app,
        &owner,
        "nary_apply",
        json!({"apply_id": "0".repeat(24)}),
    )
    .await;
    assert_eq!(wrong["applied"], json!(false));
    let (done, err) = call(
        app,
        &owner,
        "nary_apply",
        json!({"apply_id": first["apply_id"]}),
    )
    .await;
    assert!(!err && done["applied"] == json!(true), "{done}");
    assert_eq!(lead_title(app, lead).await, "Approved over MCP");
    // `finally`: the setting back (no other test of this file reads it).
    app.set_setting("nary.owner.confirm_twice_actions", &old)
        .await;
}

#[tokio::test]
async fn test_resources_and_the_prompt() {
    let app = app(TAG);
    let sa = app.superadmin().await.token;
    let h = token(app, &sa, &["read"]).await;
    let uris = |v: Value| -> Vec<String> {
        v["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["uri"].as_str().unwrap().to_string())
            .collect()
    };
    let got = uris(rpc(app, &h, "resources/list", None).await);
    for want in [
        "granary://rules",
        "granary://erp-guide",
        "granary://catalog",
        "granary://skills",
    ] {
        assert!(got.iter().any(|g| g == want), "{want}");
    }
    assert!(!got.iter().any(|g| g == "granary://style-guide"));
    let owner = token(app, &sa, &["read", "owner"]).await;
    assert!(uris(rpc(app, &owner, "resources/list", None).await)
        .iter()
        .any(|g| g == "granary://style-guide"));
    let rules = rpc(
        app,
        &h,
        "resources/read",
        Some(json!({"uri": "granary://rules"})),
    )
    .await;
    assert!(!rules["result"]["contents"][0]["text"]
        .as_str()
        .unwrap()
        .trim()
        .is_empty());
    assert_eq!(
        rpc(
            app,
            &h,
            "resources/read",
            Some(json!({"uri": "granary://style-guide"}))
        )
        .await["error"]["code"],
        -32002
    );
    assert_eq!(
        rpc(
            app,
            &h,
            "resources/read",
            Some(json!({"uri": "granary://nope"}))
        )
        .await["error"]["code"],
        -32002
    );
    let prompt = rpc(
        app,
        &h,
        "prompts/get",
        Some(json!({"name": "granary_assistant"})),
    )
    .await;
    assert!(!prompt["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .trim()
        .is_empty());

    // Python monkeypatches catalog_digest.digest and records the detail level asked for; here the text
    // each caller gets is compared with the real digest at each level ("the full catalog is for cost
    // viewers only"). A priced product with a cost makes the two levels differ, so the comparison can fail.
    let product = Uuid::new_v4();
    {
        let mut db = app.db().await;
        sqlx::query(
            "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, is_active, sale_model, price_metric, \
             modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) \
             VALUES ($1, $2, 'Pytest Laptop', 'physical', 'Unit', 5000000, true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
        )
        .bind(product)
        .bind(u("PYT"))
        .execute(&mut db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', 10000000, '2000-01-01')")
            .bind(Uuid::new_v4())
            .bind(product)
            .execute(&mut db)
            .await
            .unwrap();
    }
    let catalog = |v: Value| {
        v["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let admin_text = catalog(
        rpc(
            app,
            &h,
            "resources/read",
            Some(json!({"uri": "granary://catalog"})),
        )
        .await,
    );
    let person = app.user_with(&["crm:view_own"]).await;
    let staff_h = token(app, &person.token, &["read"]).await;
    let staff_text = catalog(
        rpc(
            app,
            &staff_h,
            "resources/read",
            Some(json!({"uri": "granary://catalog"})),
        )
        .await,
    );
    let url = app.db_url.clone();
    let (full, list) = app
        .on_server(async move {
            let mut c = PgConnection::connect(&url).await.unwrap();
            let full = granary_services::ai::catalog_digest::digest(&mut c, "full")
                .await
                .unwrap();
            let list = granary_services::ai::catalog_digest::digest(&mut c, "list")
                .await
                .unwrap();
            (full, list)
        })
        .await;
    let mut db = app.db().await;
    sqlx::query("DELETE FROM catalog_prices WHERE product_id = $1")
        .bind(product)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM product_catalog WHERE id = $1")
        .bind(product)
        .execute(&mut db)
        .await
        .unwrap();
    assert_ne!(full, list, "the full catalog is for cost viewers only");
    assert_eq!(admin_text, full);
    assert_eq!(staff_text, list);
}

#[tokio::test]
async fn test_admin_sees_and_revokes_every_connection_a_person_only_their_own() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let person = app.user_with(&["crm:view_own"]).await;
    let staff = person.token.as_str();
    let h = token(app, staff, &["read"]).await;
    assert_eq!(
        app.get(staff, &format!("{API}/mcp/connections?all=true"))
            .await
            .status,
        403
    );
    let mine = app
        .get(staff, &format!("{API}/mcp/connections"))
        .await
        .json()["items"]
        .clone();
    let mine = mine.as_array().unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["user_id"], json!(person.id.to_string()));
    let _admin_own = token(app, &sa.token, &["read"]).await;
    let admins = app
        .get(&sa.token, &format!("{API}/mcp/connections"))
        .await
        .json()["items"]
        .clone();
    let admins = admins.as_array().unwrap();
    assert!(admins
        .iter()
        .all(|c| c["user_id"] != json!(person.id.to_string())));
    let first = admins[0]["id"].as_str().unwrap();
    assert_eq!(
        app.delete(staff, &format!("{API}/mcp/connections/{first}"))
            .await
            .status,
        404
    );
    let everyone = app
        .get(&sa.token, &format!("{API}/mcp/connections?all=true"))
        .await
        .json()["items"]
        .clone();
    let theirs: Vec<&Value> = everyone
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["user_id"] == json!(person.id.to_string()))
        .collect();
    assert!(!theirs.is_empty());
    let tid = theirs[0]["id"].as_str().unwrap();
    assert_eq!(
        app.delete(&sa.token, &format!("{API}/mcp/connections/{tid}"))
            .await
            .status,
        204
    );
    let r = app
        .post(
            &h,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
    assert_eq!(r.status, 401);
}

#[tokio::test]
async fn test_a_token_from_before_access_levels_acts_as_before() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let raw = format!(
        "gpat_{}",
        granary_compat::crypto::tokens::token_urlsafe(
            &mut granary_compat::crypto::tokens::OsRandom,
            32
        )
    );
    let mut db = app.db().await;
    // PersonalAccessToken(scopes=None): SQLAlchemy writes JSON null.
    sqlx::query(
        "INSERT INTO personal_access_tokens (id, user_id, name, token_prefix, token_hash, scopes, kind)          VALUES ($1, $2, $3, $4, $5, 'null'::jsonb, 'pat')",
    )
    .bind(Uuid::new_v4())
    .bind(sa.id)
    .bind(u("legacy"))
    .bind(&raw[..16])
    .bind(granary_compat::crypto::hashes::sha256_hex(raw.as_bytes()))
    .execute(&mut db)
    .await
    .unwrap();
    let lead = lead(app).await;
    let (body, err) = call(
        app,
        &raw,
        "update_crm_record",
        json!({"object_type": "lead", "record_id": lead.to_string(), "fields": {"title": "Legacy write"}}),
    )
    .await;
    assert!(!err && body["applied"] == json!(true), "{body}");
    assert_eq!(lead_title(app, lead).await, "Legacy write");
}

/// test_nary_p1_platform.py::test_mcp_writes_go_through_the_registry.
#[tokio::test]
async fn test_mcp_writes_go_through_the_registry() {
    let app = app(TAG);
    let sa = app.superadmin().await.token;
    let lead = lead(app).await;
    let mut call_body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "update_crm_record",
                   "arguments": {"object_type": "lead", "record_id": lead.to_string(), "fields": {"title": "Via MCP"}}}});
    let act = token(app, &sa, &["act"]).await;
    let res = app
        .post(&act, &format!("{API}/mcp"), call_body.clone())
        .await;
    assert!(res.json().get("error").is_none(), "{}", res.text());
    // act: applied at once
    assert_eq!(lead_title(app, lead).await, "Via MCP");
    call_body["params"]["arguments"]["fields"] = json!({"title": "Second"});
    let propose = token(app, &sa, &["propose"]).await;
    let res = app.post(&propose, &format!("{API}/mcp"), call_body).await;
    let body: Value =
        serde_json::from_str(res.json()["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["status"], "pending");
    assert_eq!(lead_title(app, lead).await, "Via MCP");
    let mut db = app.db().await;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT origin, status FROM agent_pending_actions WHERE action = 'mcp.crm_write' \
         AND payload->>'record_id' = $1",
    )
    .bind(lead.to_string())
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(rows, vec![("mcp".to_string(), "pending".to_string())]);
    // the act write is audited as the person with origin "mcp"
    let origin: String = sqlx::query_scalar(
        "SELECT origin FROM audit_logs WHERE entity_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(lead.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(origin, "mcp");
}

/// test_assistant_access.py::test_a_portal_user_cannot_mint_an_mcp_token.
#[tokio::test]
async fn test_a_portal_user_cannot_mint_an_mcp_token() {
    let app = app(TAG);
    let vendor = app.make_user(UserSpec::new().portal("vendor")).await;
    let res = app
        .post(
            &vendor.token,
            &format!("{API}/mcp/tokens"),
            json!({"name": "sneaky"}),
        )
        .await;
    assert_eq!(res.status, 403);
}
