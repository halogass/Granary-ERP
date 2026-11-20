//! 1 Nov 2026 (Owner decisions), through the real app: direct proposal authoring over MCP.
//!
//! - tools/list: the authoring tools at act and full power, not at read or propose (a call below act is refused);
//! - the Owner's full-power round trip: proposal_create, figure_place (the drawn figure comes back as an MCP image
//!   item), proposal_set_block with a price sentence in Terms (applied at once, logged with origin mcp),
//!   proposal_check; every call is committed by the router (no card, no nary_apply).

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::storage::{self, ObjectStore, StoreError};
use serde_json::{json, Value};
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "proposal_authoring_routes";
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

async fn a_deal(app: &TestApp) -> Uuid {
    let mut db = app.db().await;
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, is_deleted) VALUES ($1, $2, false)")
        .bind(org)
        .bind(u("Globex"))
        .execute(&mut db)
        .await
        .unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, organization_id, stage, currency, value, probability, custom, is_deleted) \
         VALUES ($1, $2, $3, 'qualified', 'JPY', 0, 10, '{}', false)",
    )
    .bind(id)
    .bind(u("Globex SDK"))
    .bind(org)
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
            json!({"name": u("author"), "scopes": scopes, "password": "pytest-pass"}),
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

/// (the answer's JSON, the content items, isError).
async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, Vec<Value>, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await["result"]
        .clone();
    let items = res["content"].as_array().cloned().unwrap_or_default();
    let text = items[0]["text"].as_str().unwrap_or("").to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, items, res.get("isError") == Some(&json!(true)))
}

async fn names(app: &TestApp, h: &str) -> Vec<String> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn gantt() -> Value {
    json!({"kind": "gantt", "title": "Plan", "language": "en", "categories": [], "series": [],
           "tasks": [{"name": "Install", "start": 0, "length": 2, "group": null}],
           "milestones": [], "time_unit": "week",
           "source": {"type": "customer", "note": "the customer's schedule"}})
}

#[tokio::test]
async fn test_direct_authoring_over_mcp_at_full_power() {
    let app = app(TAG);
    let _g = app.serial().await;
    storage::set_store(Some(Arc::new(MemStore::default())));
    owner(app).await;
    let sa = app.superadmin().await.token;
    let deal = a_deal(app).await;

    for lvl in ["read", "propose"] {
        let h = token(app, &sa, &[lvl]).await;
        let listed = names(app, &h).await;
        for t in [
            "proposal_create",
            "figure_place",
            "proposal_set_block",
            "proposal_check",
        ] {
            assert!(!listed.iter().any(|n| n == t), "{t} at {lvl}");
        }
        let (_, items, err) = call(
            app,
            &h,
            "proposal_create",
            json!({"deal_id": deal.to_string(), "title": "x", "sections": []}),
        )
        .await;
        assert!(err, "{items:?}");
    }

    let full = token(app, &sa, &["full"]).await;
    let listed = names(app, &full).await;
    for t in [
        "proposal_get",
        "proposal_create",
        "proposal_set_section",
        "proposal_set_block",
        "proposal_delete_block",
        "proposal_set_requirements",
        "figure_place",
        "figure_replace",
        "figure_remove",
        "proposal_add_image",
        "proposal_check",
    ] {
        assert!(listed.iter().any(|n| n == t), "{t} at full power");
    }

    let (made, _, err) = call(
        app,
        &full,
        "proposal_create",
        json!({"deal_id": deal.to_string(), "title": "Globex SDK", "language": "en", "sections": [
            {"key": "executive_summary", "blocks": [{"type": "text", "md": "Globex gets the SDK."}]}]}),
    )
    .await;
    assert!(!err && made["applied"] == json!(true), "{made}");
    let doc = Uuid::parse_str(made["document_id"].as_str().unwrap()).unwrap();
    // committed by the router: the app's own connection sees it
    let mut db = app.db().await;
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sales_document_blocks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(n >= 3, "{n}");

    let (placed, items, err) = call(
        app,
        &full,
        "figure_place",
        json!({"document_id": doc.to_string(), "section": "implementation", "spec": gantt(),
               "caption": "Plan"}),
    )
    .await;
    assert!(!err && placed["applied"] == json!(true), "{placed}");
    assert_eq!(
        items[1]["type"],
        json!("image"),
        "the drawn figure comes back"
    );
    assert_eq!(
        placed["required_figures"]["missing"],
        json!(["architecture"])
    );

    let (set, _, err) = call(
        app,
        &full,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "section": "terms",
               "md": "The Owner-approved launch discount of JPY 25,000,000 applies to orders this month."}),
    )
    .await;
    assert!(!err && set["applied"] == json!(true), "{set}");
    let origin: Option<String> = sqlx::query_scalar(
        "SELECT origin FROM audit_logs WHERE action = 'proposal.price_text' AND entity_id = $1",
    )
    .bind(doc.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(origin.as_deref(), Some("mcp"));

    let (check, _, err) = call(
        app,
        &full,
        "proposal_check",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    assert!(!err, "{check}");
    assert_eq!(check["ready_for_approval"], json!(false));
    assert_eq!(
        check["required_figures"]["missing"],
        json!(["architecture"])
    );
}
