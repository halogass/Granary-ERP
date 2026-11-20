//! Shared fixtures for the verification delivery / signing_flow tests (Python: test_approval_engine helpers,
//! test_document_delivery fakes). One fake HTTP server stands in for the PDF service (`POST /render`), MinIO
//! (S3 paths under the documents bucket) and the WhatsApp service (`POST /send-media`), on its own thread so
//! every test runtime can use it. Tests in a file run one at a time (`serial()`): the seams are global.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::{Duration, Utc};
use granary_core::auth::AuthUser;
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

#[path = "../support/mod.rs"]
mod support;

pub static STORE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
pub static WA_SENT: Mutex<Vec<Value>> = Mutex::new(Vec::new());
pub static PDF_BYTES: Mutex<Vec<u8>> = Mutex::new(Vec::new());
pub static PDF_DOWN: AtomicBool = AtomicBool::new(false);

struct Fake;

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path().to_string();
        let method = req.method.as_str().to_string();
        if path == "/render" {
            if PDF_DOWN.load(Ordering::SeqCst) {
                return ResponseTemplate::new(503).set_body_string("connection refused");
            }
            return ResponseTemplate::new(200).set_body_bytes(PDF_BYTES.lock().unwrap().clone());
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
        // S3: /<bucket> (HEAD/PUT) and /<bucket>/<key> (PUT/GET)
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

/// Starts the fake server once (its own thread and runtime) and points the env at it.
pub fn fake_server() -> &'static str {
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
        // 3 Nov 2026: the outbound seal signs with a self-signed certificate made once in a scratch path
        std::env::set_var(
            "SIGN_CERT_P12_PATH",
            std::env::temp_dir().join(format!("granary-services-verif-{}.p12", std::process::id())),
        );
        uri
    })
}

/// One test at a time (process-wide seams, the shared fake server).
pub async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let g = LOCK.lock().await;
    WA_SENT.lock().unwrap().clear();
    PDF_DOWN.store(false, Ordering::SeqCst);
    *PDF_BYTES.lock().unwrap() = b"%PDF-1.4 verified copy".to_vec();
    g
}

/// The file's database (created once, migrated and seeded); each test connects on its own runtime.
pub async fn db_url(tag: &str) -> String {
    static URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    fake_server();
    URL.get_or_init(|| async {
        let pool = support::fresh_db(tag).await;
        pool.close().await;
        std::env::var("DATABASE_URL").unwrap()
    })
    .await
    .clone()
}

/// A connection inside a transaction (like an AsyncSession), as the services expect.
pub async fn session(tag: &str) -> PgConnection {
    let url = db_url(tag).await;
    // Nary's own sessions (an Owner alert when a seal fails) on this test's runtime: a pool made on an earlier
    // test's runtime would hang until its acquire timeout
    granary_services::nary::set_session_pool(sqlx::PgPool::connect_lazy(&url).unwrap());
    let mut c = PgConnection::connect(&url).await.unwrap();
    sqlx::query("BEGIN").execute(&mut c).await.unwrap();
    c
}

pub async fn commit(c: &mut PgConnection) {
    sqlx::query("COMMIT").execute(&mut *c).await.unwrap();
    sqlx::query("BEGIN").execute(&mut *c).await.unwrap();
}

pub fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// `make_person(roles=..., perms=..., superadmin=...)` (test_approval_engine.py), as the AuthUser a request
/// would carry.
pub async fn make_person(
    db: &mut PgConnection,
    roles: &[&str],
    perms: &[&str],
    superadmin: bool,
) -> AuthUser {
    let id = Uuid::new_v4();
    let email = format!("{}@granary.test", u("pytest.approver"));
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password, password_changed_at) \
         VALUES ($1, $2, $3, 'x', true, $4, false, 'internal', true, false, now())",
    )
    .bind(id)
    .bind(&email)
    .bind(u("Pytest Person"))
    .bind(superadmin)
    .execute(&mut *db)
    .await
    .unwrap();
    for slug in roles {
        let role: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM roles WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&mut *db)
            .await
            .unwrap();
        let rid = match role {
            Some((r,)) => r,
            None => {
                let r = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $2, true)",
                )
                .bind(r)
                .bind(slug)
                .execute(&mut *db)
                .await
                .unwrap();
                r
            }
        };
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(id)
            .bind(rid)
            .execute(&mut *db)
            .await
            .unwrap();
    }
    if !perms.is_empty() {
        let rid = Uuid::new_v4();
        let slug = u("pytest_perms");
        sqlx::query(
            "INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $2, false)",
        )
        .bind(rid)
        .bind(&slug)
        .execute(&mut *db)
        .await
        .unwrap();
        for code in perms {
            let p: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM permissions WHERE code = $1")
                .bind(code)
                .fetch_optional(&mut *db)
                .await
                .unwrap();
            let pid = match p {
                Some((p,)) => p,
                None => {
                    let p = Uuid::new_v4();
                    sqlx::query(
                        "INSERT INTO permissions (id, code, name, module) VALUES ($1, $2, $2, $3)",
                    )
                    .bind(p)
                    .bind(code)
                    .bind(code.split(':').next().unwrap())
                    .execute(&mut *db)
                    .await
                    .unwrap();
                    p
                }
            };
            sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
                .bind(rid)
                .bind(pid)
                .execute(&mut *db)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(id)
            .bind(rid)
            .execute(&mut *db)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO employees (id, first_name, email, status, user_id, salary_currency, id_type, is_deleted) \
         SELECT $1, name, email, 'active', id, 'JPY', 'national_id', false FROM users WHERE id = $2",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
    let mut user = granary_core::auth::load_active_user(&mut *db, id)
        .await
        .unwrap()
        .unwrap();
    user.perms = granary_core::auth::query_permissions(&mut *db, id)
        .await
        .unwrap();
    user
}

/// `make_product(price=10_000_000)`: a catalog item with an JPY list price.
pub async fn make_product(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, is_active, sale_model, price_metric, \
         modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) \
         VALUES ($1, $2, 'Pytest Laptop', 'physical', 'Unit', 5000000, true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
    )
    .bind(id)
    .bind(u("PYT"))
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

/// `make_quotation(product_id=..., qty=2, unit_price=10_000_000)`.
pub async fn make_quotation(db: &mut PgConnection, product_id: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    let today = Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, total_quantity, validity_days, \
         procurement_type, customer_po_accepted_verbally, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, 20000000, 20000000, '{\"text\": \"NET30\"}', 0, 0, 0, '[]', 0, 0, 14, \
         'standard', false, false)",
    )
    .bind(id)
    .bind(u("PYTEST-Q"))
    .bind(today)
    .bind(today + Duration::days(14))
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
    id
}

/// A CRM contact (and optionally its organization).
pub async fn make_contact(
    db: &mut PgConnection,
    first: &str,
    last: Option<&str>,
    whatsapp: Option<&str>,
    org: Option<Uuid>,
) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let email = format!("{}@customer.test", u(&first.to_lowercase()));
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, email, whatsapp, organization_id, custom, is_primary, tags, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, '{}', false, '{}', false)",
    )
    .bind(id)
    .bind(first)
    .bind(last)
    .bind(&email)
    .bind(whatsapp)
    .bind(org)
    .execute(&mut *db)
    .await
    .unwrap();
    (id, email)
}

pub async fn make_org(
    db: &mut PgConnection,
    name: &str,
    email: Option<&str>,
    phone: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, email, phone, custom, type, revenue_currency, tags, is_deleted) \
         VALUES ($1, $2, $3, $4, '{}', '{prospect}', 'USD', '{}', false)")
        .bind(id)
        .bind(name)
        .bind(email)
        .bind(phone)
        .execute(&mut *db)
        .await
        .unwrap();
    id
}

/// The seeded superadmin (SUPERADMIN_EMAIL): the Owner, who approves their own submissions on the spot
/// (Python's `superadmin_token` / `client`).
pub async fn owner(db: &mut PgConnection) -> AuthUser {
    let email = granary_core::config::settings().SUPERADMIN_EMAIL.clone();
    let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let mut user = granary_core::auth::load_active_user(&mut *db, id)
        .await
        .unwrap()
        .unwrap();
    user.perms = granary_core::auth::query_permissions(&mut *db, id)
        .await
        .unwrap();
    user
}

pub fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}
