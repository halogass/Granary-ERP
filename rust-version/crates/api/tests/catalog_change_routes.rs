//! 23 Oct 2026, catalog changes from your AI client (MCP), through the real app. The chat paths are
//! crates/services/tests/catalog_change.rs.
//!
//! - The Owner's connection (scopes ["full"], and ["act", "owner"]): propose_catalog_change never applies in one
//!   call: the first call returns the final version and a confirm code and nothing changes; the second call with
//!   the code applies (the price list gets a new version); the code is never applied twice.
//! - The Owner at propose level: a pending card, nothing changes.
//! - An Executive (catalog editor, not the Owner): the tool is not listed and can't be called; get_catalog_item
//!   is.
//! - The editor's routes still answer as before after the move to granary_services::catalog::writes (a price row
//!   and tiers through REST).
//!
//! One wiremock server stands in for the PDF service and MinIO, pointed at by the env before the app starts.

mod support;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use support::{u, TestApp};
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

const TAG: &str = "catalog_change";
const API: &str = "/api/v1";

static STORE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);

struct Fake;

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path().to_string();
        let method = req.method.as_str().to_string();
        if path == "/render" {
            let mut d = granary_services::pdftools::Document::new().unwrap();
            d.new_page(595.0, 842.0).unwrap();
            return ResponseTemplate::new(200)
                .set_body_bytes(d.tobytes(Default::default()).unwrap());
        }
        let mut store = STORE.lock().unwrap();
        let store = store.get_or_insert_with(HashMap::new);
        let rest = path.trim_start_matches('/');
        match (method.as_str(), rest.split_once('/')) {
            ("HEAD", None) | ("PUT", None) => ResponseTemplate::new(200),
            ("PUT", Some((_, key))) => {
                store.insert(key.to_string(), req.body.clone());
                ResponseTemplate::new(200)
            }
            ("GET", Some((_, key))) => match store.get(key) {
                Some(b) => ResponseTemplate::new(200).set_body_bytes(b.clone()),
                None => ResponseTemplate::new(404),
            },
            _ => ResponseTemplate::new(404),
        }
    }
}

fn app() -> &'static TestApp {
    static URI: OnceLock<String> = OnceLock::new();
    URI.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let server = wiremock::MockServer::start().await;
                wiremock::Mock::given(wiremock::matchers::any())
                    .respond_with(Fake)
                    .mount(&server)
                    .await;
                tx.send(server.uri()).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let uri = rx.recv().unwrap();
        std::env::set_var("PDF_SERVICE_URL", &uri);
        std::env::set_var("MINIO_ENDPOINT", uri.trim_start_matches("http://"));
        std::env::set_var("MINIO_USE_SSL", "false");
        std::env::set_var("APP_URL", "http://test.local");
        uri
    });
    support::app(TAG)
}

// ─── Fixtures ───────────────────────────────────────────────────────────────────

/// The seeded superadmin holds the Owner role (Nary's approver); its token.
async fn owner(app: &TestApp) -> support::TestUser {
    let email = std::env::var("SUPERADMIN_EMAIL").unwrap();
    let mut db = app.db().await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(id)
    .execute(&mut db)
    .await
    .unwrap();
    // the test password (an acting MCP token asks for it)
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(id)
        .bind(granary_compat::crypto::password::hash_password(support::DEFAULT_PASSWORD).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    support::TestUser {
        id,
        email,
        token: app.token_for(id),
        role_id: None,
    }
}

/// A product with an JPY price from 2000-01-01 and cost 9,000,000: (id, sku).
async fn product(app: &TestApp) -> (Uuid, String) {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    let sku = u("MCP");
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, family, is_active, is_orderable, \
         pricing_mode, sale_model, price_metric, modules, addon_for, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, 'physical', 'Unit', 9000000, 'laptop_mini', true, true, 'list', 'one_time', 'unit', \
         '{}', '{}', false, false)",
    )
    .bind(id)
    .bind(&sku)
    .bind(u("Lane Box"))
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', 15000000, '2000-01-01')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .execute(&mut db)
    .await
    .unwrap();
    (id, sku)
}

async fn cost(app: &TestApp, id: Uuid) -> Option<rust_decimal::Decimal> {
    let mut db = app.db().await;
    sqlx::query_scalar("SELECT cost_price FROM product_catalog WHERE id = $1")
        .bind(id)
        .fetch_one(&mut db)
        .await
        .unwrap()
}

async fn price_rows(app: &TestApp, id: Uuid) -> i64 {
    let mut db = app.db().await;
    sqlx::query_scalar("SELECT count(*) FROM catalog_prices WHERE product_id = $1")
        .bind(id)
        .fetch_one(&mut db)
        .await
        .unwrap()
}

async fn price_list_versions(app: &TestApp) -> i64 {
    let mut db = app.db().await;
    sqlx::query_scalar(
        "SELECT count(*) FROM library_document_versions v JOIN library_documents d ON d.id = v.document_id \
         WHERE d.kind = 'price_list'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap()
}

// ─── MCP calls ──────────────────────────────────────────────────────────────────

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("catalog"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, h: &str, method: &str, params: Value) -> Value {
    let r = app
        .post(
            h,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

/// tools/call: (the JSON of the text item, isError).
async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await["result"]
        .clone();
    let text = res["content"][0]["text"].as_str().unwrap_or("").to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, res.get("isError") == Some(&json!(true)))
}

async fn tools(app: &TestApp, h: &str) -> Vec<Value> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .clone()
}

fn names(list: &[Value]) -> Vec<String> {
    list.iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_owner_at_full_power_confirms_a_price_change_with_the_code() {
    let app = app();
    let _g = app.serial().await;
    let sa = owner(app).await;
    let full = token(app, &sa.token, &["full"]).await;
    let listed = tools(app, &full).await;
    let spec = listed
        .iter()
        .find(|t| t["name"] == "propose_catalog_change")
        .expect("listed for the Owner");
    // Security 26 Oct: no confirm code; nary_apply applies what the call parks
    assert!(
        spec["inputSchema"]["properties"].get("confirm").is_none(),
        "{spec}"
    );
    assert!(listed.iter().any(|t| t["name"] == "nary_apply"));
    let (pid, sku) = product(app).await;
    let versions = price_list_versions(app).await;
    let args = json!({"product": sku, "changes": [
        {"op": "add_price", "currency": "JPY", "amount": "16000000", "valid_from": "2030-01-01"}]});

    let (first, err) = call(app, &full, "propose_catalog_change", args.clone()).await;
    assert!(!err, "{first}");
    assert_eq!(first["approval_required"], json!(true), "{first}");
    assert_eq!(first["applied"], json!(false), "{first}");
    let shown = first["final_version"].as_str().unwrap();
    assert!(
        shown.contains("New price: JPY 16,000,000 from 2030-01-01"),
        "{shown}"
    );
    assert!(
        shown.contains("JPY 15,000,000 from 2000-01-01 to 2029-12-31"),
        "{shown}"
    );
    assert_eq!(price_rows(app, pid).await, 1, "nothing changed yet");

    let apply = json!({"apply_id": first["apply_id"]});
    let (done, err) = call(app, &full, "nary_apply", apply.clone()).await;
    assert!(!err && done["applied"] == json!(true), "{done}");
    assert_eq!(price_rows(app, pid).await, 2);
    assert_eq!(price_list_versions(app).await, versions + 1, "{done}");

    // the same apply_id again never applies a second time
    let (again, _) = call(app, &full, "nary_apply", apply).await;
    assert_ne!(again["applied"], json!(true), "{again}");
    assert_eq!(price_rows(app, pid).await, 2);
}

#[tokio::test]
async fn act_with_owner_is_the_same_two_step_and_propose_makes_a_card() {
    let app = app();
    let _g = app.serial().await;
    let sa = owner(app).await;
    let act_owner = token(app, &sa.token, &["act", "owner"]).await;
    let (pid, sku) = product(app).await;
    let args =
        json!({"product": sku, "changes": [{"op": "set_cost_price", "cost_price": "9500000"}]});
    let (first, err) = call(app, &act_owner, "propose_catalog_change", args.clone()).await;
    assert!(!err && first["approval_required"] == json!(true), "{first}");
    assert!(
        first["final_version"]
            .as_str()
            .unwrap()
            .contains("cost price: 9,000,000 -> 9,500,000"),
        "{first}"
    );
    assert_eq!(
        cost(app, pid).await,
        Some(rust_decimal::Decimal::from(9_000_000))
    );
    let apply = json!({"apply_id": first["apply_id"]});
    let (done, err) = call(app, &act_owner, "nary_apply", apply.clone()).await;
    assert!(!err && done["applied"] == json!(true), "{done}");
    assert_eq!(
        cost(app, pid).await,
        Some(rust_decimal::Decimal::from(9_500_000))
    );
    let (again, _) = call(app, &act_owner, "nary_apply", apply).await;
    assert_ne!(again["applied"], json!(true), "{again}");

    // propose level: a pending card, nothing changes
    let propose = token(app, &sa.token, &["propose"]).await;
    let args =
        json!({"product": sku, "changes": [{"op": "set_cost_price", "cost_price": "9700000"}]});
    let (card, err) = call(app, &propose, "propose_catalog_change", args).await;
    assert!(!err, "{card}");
    assert_eq!(card["applied"], json!(false), "{card}");
    assert!(card["card_id"].is_string(), "{card}");
    assert!(card.get("confirm").is_none(), "{card}");
    assert_eq!(
        cost(app, pid).await,
        Some(rust_decimal::Decimal::from(9_500_000))
    );
}

#[tokio::test]
async fn an_executive_does_not_get_the_tool() {
    let app = app();
    let _g = app.serial().await;
    owner(app).await;
    let exec = app
        .make_user(support::UserSpec::new().perms(&[
            "sales:view_catalog",
            "sales:manage_catalog",
            "sales:view_costs",
        ]))
        .await;
    let act = token(app, &exec.token, &["act"]).await;
    let listed = names(&tools(app, &act).await);
    assert!(
        !listed.iter().any(|n| n == "propose_catalog_change"),
        "{listed:?}"
    );
    assert!(listed.iter().any(|n| n == "get_catalog_item"), "{listed:?}");
    let (pid, sku) = product(app).await;
    let (out, err) = call(
        app,
        &act,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "retire"}]}),
    )
    .await;
    assert!(err, "{out}");
    assert!(
        out["text"]
            .as_str()
            .unwrap_or("")
            .contains("isn't available"),
        "{out}"
    );
    let (item, err) = call(app, &act, "get_catalog_item", json!({"product": sku})).await;
    assert!(!err && item["id"] == json!(pid.to_string()), "{item}");
}

#[tokio::test]
async fn the_editor_routes_answer_as_before() {
    let app = app();
    let _g = app.serial().await;
    let sa = owner(app).await;
    let (pid, _sku) = product(app).await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/catalog/{pid}/prices"),
            json!({"currency": " usd ", "amount": 1000, "valid_from": "2026-01-01"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["currency"], json!("USD"));
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/catalog/{pid}/prices"),
            json!({"currency": "USD", "amount": 1, "valid_from": "2026-01-01"}),
        )
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(
        r.json()["detail"],
        json!("There is already a USD price from 2026-01-01")
    );
    let r = app
        .put(
            &sa.token,
            &format!("{API}/sales/catalog/{pid}/tiers"),
            json!({"tiers": [{"min_qty": 1, "discount_pct": 2}, {"min_qty": 5, "discount_pct": 3}]}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert_eq!(
        r.json()["detail"],
        json!("Tiers overlap: 1\u{2013}\u{221e} and 5\u{2013}\u{221e}")
    );
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/sales/catalog/{pid}"),
            json!({"name": ""}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
}
