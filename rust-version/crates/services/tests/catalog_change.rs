//! 23 Oct 2026, catalog changes by Nary in the ERP chat (and WhatsApp): get_catalog_item, propose_catalog_change
//! and the action catalog.change (nary::catalog_changes). your AI client (MCP) is
//! crates/api/tests/catalog_change_routes.rs.
//!
//! - A price change: the card shows the old row ended the day before and the new row, the draft quotation with
//!   the product (it keeps its price), the price list and website lines; applied, the rows are exactly what the
//!   editor would write and the internal price list gets a new version.
//! - Every other op shows its before and after (details, cost price, tiers, retire); a new SKU with a
//!   price and tiers is created.
//! - Anyone but the Owner is refused at the tool and at apply, and does not see the tool.
//! - A card whose product changed since it was made is refused.
//! - catalog.change is on never_promote and confirm_twice (migration 0127) and can't be raised past Ask.

mod nary_n6_common;
mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog::{run_tool, tools_for_user};
use granary_services::nary::{actions, autonomy, catalog_changes as cc};
use granary_services::storage::{ObjectStore, StoreError};
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

/// The tests share process-wide state (the Nary session pool, the PDF fake, the store): one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct PdfService;

impl Respond for PdfService {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if req.url.path() == "/render" {
            let mut d = granary_services::pdftools::Document::new().unwrap();
            d.new_page(595.0, 842.0).unwrap();
            return ResponseTemplate::new(200)
                .set_body_bytes(d.tobytes(Default::default()).unwrap());
        }
        ResponseTemplate::new(404)
    }
}

/// The fake PDF service, started once, before anything reads the settings.
fn pdf_service() {
    static URI: OnceLock<String> = OnceLock::new();
    URI.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let server = wiremock::MockServer::start().await;
                wiremock::Mock::given(wiremock::matchers::any())
                    .respond_with(PdfService)
                    .mount(&server)
                    .await;
                tx.send(server.uri()).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let uri = rx.recv().unwrap();
        std::env::set_var("PDF_SERVICE_URL", &uri);
        uri
    });
}

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ObjectStore for MemStore {
    async fn bucket_exists(&self, _b: &str) -> Result<bool, StoreError> {
        Ok(true)
    }
    async fn make_bucket(&self, _b: &str) -> Result<(), StoreError> {
        Ok(())
    }
    async fn put_object(
        &self,
        _b: &str,
        key: &str,
        data: &[u8],
        _ct: &str,
    ) -> Result<(), StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .insert(key.to_string(), data.to_vec());
        Ok(())
    }
    async fn get_object(&self, _b: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(StoreError {
                kind: "S3Error".into(),
                message: key.into(),
            })
    }
}

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    pdf_service();
    granary_services::storage::set_store(Some(Arc::new(MemStore::default())));
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

/// A person with exactly these permissions (not a superadmin, not the Owner).
async fn person(db: &mut PgConnection, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password, password_changed_at) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal', true, false, now())",
    )
    .bind(id)
    .bind(format!("{}@example.com", u("catalog")))
    .bind(u("Executive"))
    .execute(&mut *db)
    .await
    .unwrap();
    let mut user = auth_user(db, id).await;
    user.perms = perms.iter().map(|p| p.to_string()).collect();
    user
}

async fn today(db: &mut PgConnection) -> chrono::NaiveDate {
    granary_core::clock::today_local(db).await.unwrap()
}

fn details(out: &Value) -> Vec<String> {
    out["proposal"]["details"]
        .as_array()
        .unwrap_or_else(|| panic!("a card: {out}"))
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

fn has(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

/// Applies a card's payload as the action does when the Owner confirms it.
async fn apply(db: &mut PgConnection, actor: &AuthUser, out: &Value) -> Result<Value, String> {
    let action = actions::get(cc::CHANGE).unwrap();
    let clean = action.validate(&out["proposal"]["payload"]).unwrap();
    (action.apply)(db, Some(actor), &clean)
        .await
        .map_err(|e| cc::detail(&e))
}

async fn prices(db: &mut PgConnection, pid: Uuid) -> Vec<(String, String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT currency, amount::text, valid_from::text, valid_to::text FROM catalog_prices \
         WHERE product_id = $1 ORDER BY currency, valid_from",
    )
    .bind(pid)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

async fn price_list_versions(db: &mut PgConnection) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM library_document_versions v JOIN library_documents d ON d.id = v.document_id \
         WHERE d.kind = 'price_list'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

// ─── a price change ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_price_change_shows_the_quotes_and_applies_as_the_editor_would() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("catalog_change_price").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let (pid, sku) = priced_product(db, &u("Tablet"), 5).await;
    let cust = customer(db, None, false, "en").await;
    let deal = deal_for(db, &cust, "qualified").await;
    let draft = quotation_for(
        db,
        deal,
        &cust,
        Some((pid, sku.clone(), "Tablet".into())),
        "draft",
        2,
        15_000_000,
    )
    .await;
    let accepted = quotation_for(
        db,
        deal,
        &cust,
        Some((pid, sku.clone(), "Tablet".into())),
        "accepted",
        1,
        15_000_000,
    )
    .await;
    let draft_no: String = sqlx::query_scalar("SELECT number FROM quotations WHERE id = $1")
        .bind(draft)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let accepted_no: String = sqlx::query_scalar("SELECT number FROM quotations WHERE id = $1")
        .bind(accepted)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let t = today(db).await;
    let from = t + chrono::Duration::days(1);

    // Nary reads the product first
    let item = run_tool(db, &me, "get_catalog_item", json!({"product": sku})).await;
    assert_eq!(item["sku"], json!(sku), "{item}");
    assert_eq!(item["price_rows"].as_array().unwrap().len(), 2, "{item}");
    assert!(
        item["volume_tiers"].is_array(),
        "the Owner sees the tiers: {item}"
    );

    let out = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [
            {"op": "add_price", "currency": "jpy", "amount": "16500000", "valid_from": from.to_string()}]}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], json!(cc::CHANGE), "{out}");
    assert!(
        out["proposal"].get("auto").is_none(),
        "always a card: {out}"
    );
    let lines = details(&out);
    assert!(
        has(&lines, &format!("New price: JPY 16,500,000 from {from}")),
        "{lines:#?}"
    );
    assert!(
        has(&lines, &format!("Price: JPY 15,000,000 from 2000-01-01 (open-ended) -> JPY 15,000,000 from 2000-01-01 to {t}")),
        "the open row ends the day before: {lines:#?}"
    );
    assert!(
        has(&lines, &draft_no),
        "the draft quote is listed: {lines:#?}"
    );
    assert!(
        !has(&lines, &accepted_no),
        "an accepted one is not: {lines:#?}"
    );
    assert!(has(&lines, "JPY 15,000,000 on the quote"), "{lines:#?}");
    assert!(has(&lines, cc::QUOTES_KEEP), "{lines:#?}");
    assert!(has(&lines, cc::PRICE_LIST), "{lines:#?}");
    // nothing changed yet
    assert_eq!(prices(db, pid).await.len(), 2);

    let versions = price_list_versions(db).await;
    let done = apply(db, &me, &out).await.unwrap();
    assert!(
        done["label"].as_str().unwrap().contains("Catalog updated"),
        "{done}"
    );
    assert!(
        done["price_list"].is_string(),
        "the price list is refreshed: {done}"
    );
    assert_eq!(price_list_versions(db).await, versions + 1);
    let rows = prices(db, pid).await;
    assert_eq!(
        rows,
        vec![
            (
                "JPY".into(),
                "15000000.00".into(),
                "2000-01-01".into(),
                Some(t.to_string())
            ),
            ("JPY".into(), "16500000.00".into(), from.to_string(), None),
            ("USD".into(), "1000.00".into(), "2000-01-01".into(), None),
        ]
    );
    // the quote keeps its price
    let unit: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT unit_price FROM quotation_items WHERE quotation_id = $1")
            .bind(draft)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(unit, rust_decimal::Decimal::from(15_000_000));

    // the same card again is stale now
    let again = apply(db, &me, &out).await.unwrap_err();
    assert_eq!(again, cc::STALE);

    // an add_price on the same start date is the editor's 409 text, at the tool
    let clash = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [
            {"op": "add_price", "currency": "JPY", "amount": "17000000", "valid_from": from.to_string()}]}),
    )
    .await;
    assert_eq!(
        clash["error"],
        json!(format!("There is already a JPY price from {from}")),
        "{clash}"
    );
}

// ─── every op ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn each_op_shows_before_and_after_and_applies() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("catalog_change_ops").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let (pid, sku) = priced_product(db, &u("Docking Station"), 5).await;

    let out = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": pid.to_string(), "changes": [
            {"op": "update_product", "fields": {"description": "Docking station, 2 years warranty", "min_quantity": 2}},
            {"op": "set_cost_price", "cost_price": "9500000"},
            {"op": "set_tiers", "tiers": [{"min_qty": 5, "max_qty": 9, "discount_pct": 5},
                                           {"min_qty": 10, "discount_pct": 8}]},
            {"op": "end_price", "currency": "USD"},
        ]}),
    )
    .await;
    let lines = details(&out);
    assert!(
        has(
            &lines,
            "description: Laptop with 16 GB RAM, 1 year warranty -> Docking station, 2 years warranty"
        ),
        "{lines:#?}"
    );
    assert!(has(&lines, "min quantity: (empty) -> 2"), "{lines:#?}");
    assert!(
        has(&lines, "cost price: 9,000,000 -> 9,500,000"),
        "{lines:#?}"
    );
    assert!(
        has(&lines, "Volume tiers (private): 5+: 5% -> 5-9: 5%, 10+: 8%"),
        "{lines:#?}"
    );
    let t = today(db).await;
    assert!(
        has(
            &lines,
            &format!(
                "Price: USD 1,000 from 2000-01-01 (open-ended) -> USD 1,000 from 2000-01-01 to {t}"
            )
        ),
        "{lines:#?}"
    );
    apply(db, &me, &out).await.unwrap();
    let row: (Option<String>, Option<rust_decimal::Decimal>) =
        sqlx::query_as("SELECT description, cost_price FROM product_catalog WHERE id = $1")
            .bind(pid)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(row.0.as_deref(), Some("Docking station, 2 years warranty"));
    assert_eq!(row.1, Some(rust_decimal::Decimal::from(9_500_000)));
    let tiers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_volume_tiers WHERE product_id = $1")
            .bind(pid)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(tiers, 2);

    // retire, then unretire
    let out = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "retire"}]}),
    )
    .await;
    assert!(has(&details(&out), "Status: active -> retired"), "{out}");
    apply(db, &me, &out).await.unwrap();
    let flags: (Option<bool>, Option<bool>, bool) = sqlx::query_as(
        "SELECT is_active, is_orderable, is_deleted FROM product_catalog WHERE id = $1",
    )
    .bind(pid)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(flags, (Some(false), Some(false), false));
    let out = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "unretire"}]}),
    )
    .await;
    assert!(has(&details(&out), "Status: retired -> active"), "{out}");

    // the editor's checks apply: a recurring item needs rate_period_months
    let bad = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "update_product", "fields": {"sale_model": "recurring"}}]}),
    )
    .await;
    assert_eq!(
        bad["error"],
        json!("A recurring item needs rate_period_months (the period its price is quoted per)"),
        "{bad}"
    );
    // an unknown field is named
    let bad = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "update_product", "fields": {"colour": "red"}}]}),
    )
    .await;
    assert!(
        bad["error"]
            .as_str()
            .unwrap()
            .contains("colour is not a product field"),
        "{bad}"
    );
}

#[tokio::test]
async fn a_new_sku_with_its_price_and_tiers() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("catalog_change_new").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let sku = u("NEW");
    let out = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"changes": [
            {"op": "create_product", "fields": {"sku": sku, "name": "Parking Analytics", "product_type": "software",
                                                "family": "parking"}},
            {"op": "add_price", "currency": "JPY", "amount": "25000000", "valid_from": "2026-01-01"},
            {"op": "set_tiers", "tiers": [{"min_qty": 3, "discount_pct": 4}]},
        ]}),
    )
    .await;
    let lines = details(&out);
    assert!(
        has(&lines, &format!("New product: Parking Analytics ({sku})")),
        "{lines:#?}"
    );
    assert!(has(&lines, "family: parking"), "{lines:#?}");
    assert!(
        has(
            &lines,
            "New price: JPY 25,000,000 from 2026-01-01 (open-ended)"
        ),
        "{lines:#?}"
    );
    assert!(
        has(&lines, "Volume tiers (private): none -> 3+: 4%"),
        "{lines:#?}"
    );
    let none: Option<Uuid> = sqlx::query_scalar("SELECT id FROM product_catalog WHERE sku = $1")
        .bind(&sku)
        .fetch_optional(&mut *db)
        .await
        .unwrap();
    assert!(none.is_none(), "nothing saved before the confirm");
    let done = apply(db, &me, &out).await.unwrap();
    let pid = Uuid::parse_str(done["product_id"].as_str().unwrap()).unwrap();
    let row: (String, String, String, Option<String>, Option<Vec<String>>) = sqlx::query_as(
        "SELECT name, product_type, sale_model, unit, modules FROM product_catalog WHERE id = $1",
    )
    .bind(pid)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    // the editor's defaults: one_time, unit "Unit", modules []
    assert_eq!(
        row,
        (
            "Parking Analytics".into(),
            "software".into(),
            "one_time".into(),
            Some("Unit".into()),
            Some(vec![])
        )
    );
    assert_eq!(prices(db, pid).await.len(), 1);
    // the same SKU again is the editor's 409 text
    let again = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"changes": [{"op": "create_product", "fields": {"sku": sku, "name": "Again"}}]}),
    )
    .await;
    assert_eq!(
        again["error"],
        json!(format!("SKU {sku} already exists")),
        "{again}"
    );
}

// ─── who may ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn only_the_owner_may_and_a_stale_card_is_refused() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("catalog_change_who").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let (pid, sku) = priced_product(db, &u("Monitor"), 5).await;
    let exec = person(
        db,
        &[
            "sales:view_catalog",
            "sales:manage_catalog",
            "sales:view_costs",
            "nary:approve",
        ],
    )
    .await;

    // the tool is in the plain list (permission gate), but the chat and MCP lists drop it for a non-Owner
    let mut listed = tools_for_user(&exec);
    assert!(listed.iter().any(|t| t.name == "propose_catalog_change"));
    granary_services::ai::tools::catalog::catalog_change_tools::keep_for_owner(
        db,
        &exec,
        &mut listed,
    )
    .await
    .unwrap();
    assert!(!listed.iter().any(|t| t.name == "propose_catalog_change"));
    assert!(listed.iter().any(|t| t.name == "get_catalog_item"));
    let mut mine = tools_for_user(&me);
    granary_services::ai::tools::catalog::catalog_change_tools::keep_for_owner(db, &me, &mut mine)
        .await
        .unwrap();
    assert!(mine.iter().any(|t| t.name == "propose_catalog_change"));

    let refused = run_tool(
        db,
        &exec,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "set_cost_price", "cost_price": "1"}]}),
    )
    .await;
    assert_eq!(refused["error"], json!(cc::ONLY_OWNER), "{refused}");
    // the Executive reads the item without the tiers
    let item = run_tool(db, &exec, "get_catalog_item", json!({"product": sku})).await;
    assert!(item.get("volume_tiers").is_none(), "{item}");
    assert!(
        item["cost_price"].is_string(),
        "a cost viewer sees the cost: {item}"
    );

    let out = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "set_cost_price", "cost_price": "9100000"}]}),
    )
    .await;
    // at apply: the Executive is refused
    assert_eq!(apply(db, &exec, &out).await.unwrap_err(), cc::ONLY_OWNER);
    // the product changes in the editor meanwhile: the card is stale
    let product = granary_services::catalog::writes::load_product(db, &pid.to_string())
        .await
        .unwrap();
    let mut f: granary_services::catalog::writes::ProductFields =
        granary_core::validation::validate_value(
            &granary_core::validation::PyObj::from_json(&json!({"name": "Monitor 2"})),
            None,
        )
        .unwrap();
    f.name = Some("Monitor 2".into());
    granary_services::catalog::writes::update_product(db, &product, &f, &["name".to_string()])
        .await
        .unwrap();
    assert_eq!(apply(db, &me, &out).await.unwrap_err(), cc::STALE);
    let cost: Option<rust_decimal::Decimal> =
        sqlx::query_scalar("SELECT cost_price FROM product_catalog WHERE id = $1")
            .bind(pid)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(cost, Some(rust_decimal::Decimal::from(9_000_000)));
}

#[tokio::test]
async fn catalog_change_is_never_promoted_and_confirmed_twice() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("catalog_change_autonomy").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    assert!(autonomy::never_promote(db)
        .await
        .unwrap()
        .contains(&cc::CHANGE.to_string()));
    assert!(autonomy::confirm_twice(db)
        .await
        .unwrap()
        .contains(&cc::CHANGE.to_string()));
    assert_eq!(
        autonomy::level_for(db, cc::CHANGE).await.unwrap(),
        autonomy::ASK
    );
    let raised = autonomy::set_level(db, cc::CHANGE, 3, Some(&me))
        .await
        .unwrap_err();
    assert!(cc::detail(&raised).contains("never-promote"), "{raised:?}");
    // even written into the levels by hand, it stays at Ask
    sqlx::query(
        "UPDATE system_settings SET value = jsonb_set(value::jsonb, '{catalog.change}', '3')::text \
         WHERE key = 'nary.autonomy.levels'",
    )
    .execute(&mut *db)
    .await
    .unwrap();
    assert_eq!(
        autonomy::level_for(db, cc::CHANGE).await.unwrap(),
        autonomy::ASK
    );
    let (_pid, sku) = priced_product(db, &u("Keyboard"), 5).await;
    let out = run_tool(
        db,
        &me,
        "propose_catalog_change",
        json!({"product": sku, "changes": [{"op": "retire"}]}),
    )
    .await;
    assert!(out["proposal"].get("auto").is_none(), "{out}");
    assert!(
        granary_services::nary::executor::CHAT_ACTIONS.contains(&cc::CHANGE),
        "the chat's confirm runs it"
    );
}
