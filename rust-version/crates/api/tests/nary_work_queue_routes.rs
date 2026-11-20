//! 3 Nov 2026 (Owner decisions, Stream A), through the real app with nary.paused = true:
//! - nary_work_queue over MCP: listed and callable at act and full power, refused at read and propose.
//! - GET /api/v1/nary/work-queue (Nary > Work queue): the Owner and system admins; filters; others get 403.

mod support;

use serde_json::{json, Value};
use support::{app, u, TestApp, UserSpec};
use uuid::Uuid;

const TAG: &str = "nary_work_queue_routes";
const API: &str = "/api/v1";

async fn owner(app: &TestApp) -> Uuid {
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    sa.id
}

/// A customer chat where the customer had the last word an hour ago: its chat id.
async fn waiting_chat(app: &TestApp) -> String {
    let mut db = app.db().await;
    let p = format!("8180{}", &Uuid::new_v4().as_u128().to_string()[..8]);
    let contact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, whatsapp, is_primary, tags, is_deleted, custom) \
         VALUES ($1, 'Bob', $2, $3, false, '{}', false, '{}')",
    )
    .bind(contact_id)
    .bind(u("Queue"))
    .bind(format!("+{p}"))
    .execute(&mut db)
    .await
    .unwrap();
    let chat_id = format!("{p}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, \
         contact_id, triage, language, attribution_asked, last_inbound_at) \
         VALUES ($1, $2, $3, 'Bob', false, 'active', true, $4, 'prospect', 'id', false, now() - interval '1 hour')",
    )
    .bind(Uuid::new_v4())
    .bind(&chat_id)
    .bind(format!("+{p}"))
    .bind(contact_id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts, contact_id) \
         VALUES ($1, $2, $3, 'inbound', false, 'Hello, how much is the laptop?', now() - interval '1 hour', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(&chat_id)
    .bind(contact_id)
    .execute(&mut db)
    .await
    .unwrap();
    chat_id
}

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("queue"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, h: &str, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let r = app.post(h, &format!("{API}/mcp"), body).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await["result"]
        .clone();
    let text = res["content"][0]["text"].as_str().unwrap().to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, res.get("isError") == Some(&json!(true)))
}

async fn listed(app: &TestApp, h: &str) -> Vec<String> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn test_work_queue_over_mcp_by_level_and_the_page_route() {
    let app = app(TAG);
    let _g = app.serial().await;
    app.set_setting("nary.paused", "true").await;
    owner(app).await;
    let sa = app.superadmin().await.token;
    let chat = waiting_chat(app).await;
    let item_id = format!("whatsapp_reply:{chat}");

    // read and propose: not listed, refused when called
    for lvl in ["read", "propose"] {
        let h = token(app, &sa, &[lvl]).await;
        assert!(
            !listed(app, &h).await.iter().any(|n| n == "nary_work_queue"),
            "{lvl}"
        );
        let (body, err) = call(app, &h, "nary_work_queue", json!({})).await;
        assert!(
            err && body["text"]
                .as_str()
                .unwrap_or("")
                .contains("isn't available at this connection's access level"),
            "{lvl}: {body}"
        );
    }
    // act and full power: listed, and the chat waiting is an urgent item with its next tool
    for scopes in [vec!["act"], vec!["full"]] {
        let h = token(app, &sa, &scopes).await;
        assert!(
            listed(app, &h).await.iter().any(|n| n == "nary_work_queue"),
            "{scopes:?}"
        );
        let (body, err) = call(
            app,
            &h,
            "nary_work_queue",
            json!({"urgency": "urgent", "limit": 200}),
        )
        .await;
        assert!(!err, "{scopes:?}: {body}");
        assert_eq!(body["nary_paused"], json!(true));
        let item = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == json!(item_id))
            .unwrap_or_else(|| panic!("{scopes:?}: {body}"))
            .clone();
        assert_eq!(item["kind"], json!("whatsapp_reply"));
        assert_eq!(item["suggested_tool"], json!("get_whatsapp_messages"));
        assert_eq!(item["suggested_args"], json!({"chat_id": chat}));
        // a tool error comes back as its text (isError)
        let (body, err) = call(app, &h, "nary_work_queue", json!({"kinds": ["bogus"]})).await;
        assert!(
            err && body["text"]
                .as_str()
                .unwrap_or("")
                .contains("Unknown kind 'bogus'"),
            "{body}"
        );
    }

    // the page's route: the Owner, a system admin; filters; anyone else 403
    let r = app
        .get(
            &sa,
            &format!("{API}/nary/work-queue?kinds=whatsapp_reply,new_lead&limit=500"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == json!(item_id)),
        "{body}"
    );
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["kind"] == "whatsapp_reply" || i["kind"] == "new_lead"));
    assert!(body["by_urgency"]["urgent"].as_i64().unwrap() >= 1);
    let r = app
        .get(&sa, &format!("{API}/nary/work-queue?urgency=soon"))
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    let admin = app.make_user(UserSpec::new().system_admin()).await;
    let r = app
        .get(
            &admin.token,
            &format!("{API}/nary/work-queue?urgency=urgent"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let staff = app.user_with(&["nary:view", "crm:view_all"]).await;
    let r = app
        .get(&staff.token, &format!("{API}/nary/work-queue"))
        .await;
    assert_eq!(r.status, 403, "{}", r.text());

    // answered: the item is gone
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts) \
         VALUES ($1, $2, $3, 'outbound', true, 'Hello, we are sending the prices', now())",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(&chat)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .get(
            &sa,
            &format!("{API}/nary/work-queue?kinds=whatsapp_reply&limit=500"),
        )
        .await;
    assert!(!r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["id"] == json!(item_id)));
    app.set_setting("nary.paused", "false").await;
}
