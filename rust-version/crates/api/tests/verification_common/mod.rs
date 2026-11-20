//! Shared fakes for verification_routes.rs and delivery_routes.rs: one wiremock server stands in for the PDF
//! service (`POST /render`), MinIO (S3 paths under the documents bucket) and the WhatsApp service
//! (`POST /send-media`), pointed at by the env BEFORE `support::app` starts. Plus the rows the Python
//! fixtures build (test_approval_engine.py `make_person`, `make_product`, `make_quotation`).
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

use crate::support;

pub static STORE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
pub static WA_SENT: Mutex<Vec<Value>> = Mutex::new(Vec::new());
pub static RENDERS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
pub static PDF_DOWN: AtomicBool = AtomicBool::new(false);
pub const PDF: &[u8] = b"%PDF-1.4 verified copy";

struct Fake;

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path().to_string();
        let method = req.method.as_str().to_string();
        if path == "/render" {
            if PDF_DOWN.load(Ordering::SeqCst) {
                return ResponseTemplate::new(503).set_body_string("connection refused");
            }
            RENDERS
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&req.body).unwrap_or(Value::Null));
            return ResponseTemplate::new(200).set_body_bytes(PDF.to_vec());
        }
        if path == "/send-media" {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let mut rec = json!({"url": req.url.to_string()});
            if let Value::Object(m) = body {
                for (k, v) in m {
                    rec[k] = v;
                }
            }
            WA_SENT.lock().unwrap().push(rec);
            return ResponseTemplate::new(200)
                .set_body_json(json!({"success": true, "messageId": "wamid-1"}));
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

/// Starts the fake server once and points the env at it; then the app.
pub fn app(tag: &str) -> &'static support::TestApp {
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
        std::env::set_var("WHATSAPP_SERVICE_URL", &uri);
        std::env::set_var("MINIO_ENDPOINT", uri.trim_start_matches("http://"));
        std::env::set_var("MINIO_USE_SSL", "false");
        std::env::set_var("APP_URL", "http://test.local");
        uri
    });
    support::app(tag)
}

/// `make_person(roles=..., perms=...)`.
pub async fn make_person(
    app: &support::TestApp,
    roles: &[&str],
    perms: &[&str],
) -> support::TestUser {
    app.make_user(support::UserSpec::new().perms(perms).roles(roles))
        .await
}

/// `make_product()`: a catalog item with an JPY list price of 10,000,000.
pub async fn make_product(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, is_active, sale_model, price_metric, \
         modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) \
         VALUES ($1, $2, 'Pytest Laptop', 'physical', 'Unit', 5000000, true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
    )
    .bind(id)
    .bind(support::u("PYT"))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', 10000000, '2000-01-01')")
        .bind(Uuid::new_v4())
        .bind(id)
        .execute(&mut *db)
        .await
        .unwrap();
    id
}

/// `make_quotation(product_id=..., qty=2, unit_price=10_000_000)`, optionally for a CRM contact
/// (Bob Sanders, WhatsApp +81 90 1234 5678); returns (id, contact email).
pub async fn make_quotation(
    db: &mut PgConnection,
    product_id: Option<Uuid>,
    with_contact: bool,
) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let today = chrono::Utc::now().date_naive();
    let mut email = String::new();
    let mut contact: Option<Uuid> = None;
    if with_contact {
        let cid = Uuid::new_v4();
        email = format!("{}@customer.test", support::u("bob"));
        sqlx::query(
            "INSERT INTO contacts (id, first_name, last_name, email, whatsapp, custom, is_primary, tags, is_deleted) \
             VALUES ($1, 'Bob', 'Sanders', $2, '+81 90 1234 5678', '{}', false, '{}', false)",
        )
        .bind(cid)
        .bind(&email)
        .execute(&mut *db)
        .await
        .unwrap();
        contact = Some(cid);
    }
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, total_quantity, validity_days, \
         procurement_type, customer_po_accepted_verbally, is_deleted, customer_contact_id) \
         VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, 20000000, 20000000, '{\"text\": \"NET30\"}', 0, 0, 0, '[]', 0, 0, 14, \
         'standard', false, false, $5)",
    )
    .bind(id)
    .bind(support::u("PYTEST-Q"))
    .bind(today)
    .bind(today + chrono::Duration::days(14))
    .bind(contact)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, product_id, name, quantity, unit_price, currency, \
         discount_percentage, discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, $3, 'Line', 2, 10000000, 'JPY', 0, 'none', 20000000, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(product_id)
    .execute(&mut *db)
    .await
    .unwrap();
    (id, email)
}

/// The seeded superadmin (SUPERADMIN_EMAIL): the Owner, the approver of the Owner steps, who approves their own
/// submissions on the spot (Python's `superadmin_token`).
pub async fn owner(app: &support::TestApp) -> support::TestUser {
    let email = std::env::var("SUPERADMIN_EMAIL").unwrap();
    let mut db = app.db().await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&mut db)
        .await
        .unwrap();
    support::TestUser {
        id,
        email,
        token: app.token_for(id),
        role_id: None,
    }
}

/// An approved quotation: the Owner's prepare-send approves it on the spot.
pub async fn approve_as_owner(app: &support::TestApp, entity_type: &str, id: Uuid) {
    let sa = owner(app).await;
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/documents/{entity_type}/{id}/prepare-send"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["approved"], true);
}

pub async fn status_of(db: &mut PgConnection, table: &str, id: Uuid) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT status FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// A walk-in invoice (no CRM records) with its own billing email / WhatsApp, as /finance/invoices creates it.
pub async fn make_invoice(db: &mut PgConnection, email: &str, tel: &str) -> Uuid {
    let id = Uuid::new_v4();
    let today = chrono::Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO invoices (id, number, invoice_type, revision, status, currency, exchange_rate, subtotal, tax_total, \
         total, amount_paid, amount_due, refund_due, issue_date, due_date, customer_name, customer_email, customer_tel, \
         tax_lines, is_deleted) VALUES ($1, $2, 'commercial', 1, 'draft', 'JPY', 1, 2500000, 0, 2500000, 0, 2500000, 0, \
         $3, '2026-12-31', 'Walk-in Co', $4, $5, '[]', false)",
    )
    .bind(id)
    .bind(support::u("PYT-INV"))
    .bind(today)
    .bind(email)
    .bind(tel)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO invoice_items (id, invoice_id, name, quantity, unit_price, amount, discount_pct, discount_amount, \
         tax_pct, tax_amount, sort_order, unit, currency) VALUES ($1, $2, 'Service', 1, 2500000, 2500000, 0, 0, 0, 0, 0, 'Unit', 'JPY')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}
