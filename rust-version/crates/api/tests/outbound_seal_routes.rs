//! 3 Nov 2026 (Owner): every approved outbound PDF carries ACME's digital seal. Through the real app: an approved
//! quotation's verification keeps its unsealed master and gets a sealed copy (PAdES certification, DocMDP P=2)
//! next to it; email, WhatsApp, the public view/download and the staff download all carry the sealed bytes; the
//! public JSON says it is sealed; documents.seal.outbound off sends the master, and a verification issued
//! while it was off (a legacy one) is sealed on its next send; a seal that fails sends the master and alerts
//! the Owner once.
//!
//! One wiremock server stands in for the PDF service (`/render` answers a real PDF, or junk to make the seal
//! fail), MinIO and the WhatsApp service. The seal uses a self-signed certificate generated in a scratch path.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use granary_services::email::{Capture, Seams, SendCall, SmtpConfig};
use granary_services::pdf_sign;
use granary_services::verification::delivery::seams;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

const TAG: &str = "outbound_seal";
const SEND_PERMS: &[&str] = &["approvals:view", "sales:send_quotation"];

static STORE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
static WA_SENT: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static RENDER: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static EMAILS: Mutex<Vec<SendCall>> = Mutex::new(Vec::new());

fn anchors_pdf() -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../compat-tests/golden/verification_delivery/anchors.pdf"),
    )
    .unwrap()
}

struct Fake;

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path().to_string();
        let method = req.method.as_str().to_string();
        if path == "/render" {
            let body = RENDER.lock().unwrap().clone().unwrap_or_else(anchors_pdf);
            return ResponseTemplate::new(200).set_body_bytes(body);
        }
        if path == "/send-media" {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            WA_SENT.lock().unwrap().push(body);
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

fn app() -> &'static support::TestApp {
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
        let p12 = std::env::temp_dir().join(format!(
            "granary-api-outbound-seal-{}.p12",
            std::process::id()
        ));
        std::env::set_var("PDF_SERVICE_URL", &uri);
        std::env::set_var("WHATSAPP_SERVICE_URL", &uri);
        std::env::set_var("MINIO_ENDPOINT", uri.trim_start_matches("http://"));
        std::env::set_var("MINIO_USE_SSL", "false");
        std::env::set_var("APP_URL", "http://test.local");
        std::env::set_var("SIGN_CERT_P12_PATH", p12);
        std::env::set_var("SIGN_TSA_URL", "");
        uri
    });
    support::app(TAG)
}

fn stored(key: &str) -> Option<Vec<u8>> {
    STORE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(key)
        .cloned()
}

async fn setup() -> (
    &'static support::TestApp,
    tokio::sync::MutexGuard<'static, ()>,
) {
    let app = app();
    let g = app.serial().await;
    app.ensure_crm_mailbox("sales@example.test").await;
    EMAILS.lock().unwrap().clear();
    WA_SENT.lock().unwrap().clear();
    *RENDER.lock().unwrap() = None;
    app.set_setting("documents.seal.outbound", "true").await;
    seams::set_save_to_sent(Some(Arc::new(|_raw| Box::pin(async move {}))));
    granary_services::email::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(|call: SendCall| {
            Box::pin(async move {
                let wants = call.wants_capture;
                EMAILS.lock().unwrap().push(call);
                let cap = wants.then(|| Capture {
                    raw: Some(b"raw message bytes".to_vec()),
                    message_id: None,
                });
                Ok((true, cap))
            })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: String::new(),
                    password: String::new(),
                    from_name: "Your Company".into(),
                    from_email: "sales@example.test".into(),
                    tls: true,
                })
            })
        })),
    }));
    (app, g)
}

/// The seeded superadmin (the Owner).
async fn owner(app: &support::TestApp) -> support::TestUser {
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

/// A quotation at catalog list price (auto-approved on send) for Bob Sanders; returns (id, email).
async fn quote(app: &support::TestApp) -> (Uuid, String) {
    let mut db = app.db().await;
    let product = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, is_active, sale_model, price_metric, \
         modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) \
         VALUES ($1, $2, 'Seal Laptop', 'physical', 'Unit', 5000000, true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
    )
    .bind(product)
    .bind(support::u("SEAL"))
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', 10000000, '2000-01-01')")
        .bind(Uuid::new_v4())
        .bind(product)
        .execute(&mut db)
        .await
        .unwrap();
    let contact = Uuid::new_v4();
    let email = format!("{}@customer.test", support::u("bob"));
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, email, whatsapp, custom, is_primary, tags, is_deleted) \
         VALUES ($1, 'Bob', 'Sanders', $2, '+81 90 1234 5678', '{}', false, '{}', false)",
    )
    .bind(contact)
    .bind(&email)
    .execute(&mut db)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    let today = chrono::Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, total_quantity, validity_days, \
         procurement_type, customer_po_accepted_verbally, is_deleted, customer_contact_id) \
         VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, 20000000, 20000000, '{\"text\": \"NET30\"}', 0, 0, 0, '[]', 0, 0, 14, \
         'standard', false, false, $5)",
    )
    .bind(id)
    .bind(support::u("SEAL-Q"))
    .bind(today)
    .bind(today + chrono::Duration::days(14))
    .bind(contact)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, product_id, name, quantity, unit_price, currency, \
         discount_percentage, discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, $3, 'Line', 2, 10000000, 'JPY', 0, 'none', 20000000, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(product)
    .execute(&mut db)
    .await
    .unwrap();
    // approved by the Owner (prepare-send approves on the spot), so it can be sent again after it is sent
    let sa = owner(app).await;
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/documents/quotation/{id}/prepare-send"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["approved"], true);
    (id, email)
}

/// A walk-in invoice with its own billing email and WhatsApp (the Owner's send approves it on the spot, and an
/// invoice can be sent again).
async fn invoice(app: &support::TestApp) -> (Uuid, String) {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    let email = format!("{}@cust.test", support::u("sarah"));
    let today = chrono::Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO invoices (id, number, invoice_type, revision, status, currency, exchange_rate, subtotal, tax_total, \
         total, amount_paid, amount_due, refund_due, issue_date, due_date, customer_name, customer_email, customer_tel, \
         tax_lines, is_deleted) VALUES ($1, $2, 'commercial', 1, 'draft', 'JPY', 1, 2500000, 0, 2500000, 0, 2500000, 0, \
         $3, '2026-12-31', 'Walk-in Co', $4, '+81 80 1111 2222', '[]', false)",
    )
    .bind(id)
    .bind(support::u("SEAL-INV"))
    .bind(today)
    .bind(&email)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO invoice_items (id, invoice_id, name, quantity, unit_price, amount, discount_pct, discount_amount, \
         tax_pct, tax_amount, sort_order, unit, currency) VALUES ($1, $2, 'Service', 1, 2500000, 2500000, 0, 0, 0, 0, 0, 'Unit', 'JPY')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .execute(&mut db)
    .await
    .unwrap();
    (id, email)
}

async fn send(
    app: &support::TestApp,
    token: &str,
    entity_type: &str,
    id: Uuid,
    body: Value,
) -> support::Resp {
    let r = app
        .post(
            token,
            &format!("/api/v1/documents/{entity_type}/{id}/send"),
            body.clone(),
        )
        .await;
    assert_eq!(r.status, 200, "{body}: {}", r.text());
    r
}

fn last_attachment() -> Vec<u8> {
    let mails = EMAILS.lock().unwrap().clone();
    let mail = mails.last().expect("an email was sent");
    mail.attachments.as_ref().unwrap()[0].content.clone()
}

struct Row {
    id: Uuid,
    token: Uuid,
    master_key: String,
    sealed_key: Option<String>,
    sealed_hash: Option<String>,
    level: Option<String>,
    sealed: bool,
}

async fn active_verification(app: &support::TestApp, entity_id: Uuid) -> Row {
    let mut db = app.db().await;
    #[allow(clippy::type_complexity)]
    let row: (
        Uuid,
        Uuid,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT id, token, pdf_object_key, sealed_object_key, sealed_hash, seal_level, sealed_at \
         FROM document_verifications WHERE entity_id = $1 AND status = 'active'",
    )
    .bind(entity_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    Row {
        id: row.0,
        token: row.1,
        master_key: row.2,
        sealed_key: row.3,
        sealed_hash: row.4,
        level: row.5,
        sealed: row.6.is_some(),
    }
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn assert_sealed(bytes: &[u8]) {
    assert_eq!(
        pdf_sign::docmdp_permission(bytes),
        Some(2),
        "a P=2 certification"
    );
    let r = pdf_sign::validate_pdf(bytes);
    assert_eq!((r.intact, r.valid), (Some(true), Some(true)), "{r:?}");
}

#[tokio::test]
async fn an_approved_quotation_goes_out_sealed_everywhere() {
    let (app, _g) = setup().await;
    let bdr = app
        .make_user(
            support::UserSpec::new()
                .perms(SEND_PERMS)
                .roles(&["sales_bizdev"]),
        )
        .await;
    let (qid, email) = quote(app).await;
    // issued at approval time (staff issue): sealed right away
    let sa = owner(app).await;
    let issued = app
        .post(
            &sa.token,
            "/api/v1/verify/issue",
            json!({"entity_type": "quotation", "entity_id": qid.to_string()}),
        )
        .await;
    assert_eq!(issued.status, 201, "{}", issued.text());

    let v = active_verification(app, qid).await;
    assert_eq!(
        v.sealed_key.as_deref(),
        Some(format!("verification/quotation/{}.sealed.pdf", v.id).as_str())
    );
    assert_eq!(v.level.as_deref(), Some("pades-b-b"));
    assert!(v.sealed);
    // the master stays unsealed (e-sign and the AI client's page renderer read it)
    let master = stored(&v.master_key).unwrap();
    assert_eq!(master, anchors_pdf());
    assert_eq!(pdf_sign::docmdp_permission(&master), None);
    let sealed = stored(v.sealed_key.as_deref().unwrap()).unwrap();
    assert_sealed(&sealed);
    assert_eq!(v.sealed_hash.as_deref(), Some(sha(&sealed).as_str()));
    assert!(
        sealed.starts_with(&master),
        "an incremental update of the master"
    );
    // the public page says so, and the public view and download are the sealed copy
    let meta = app.get("", &format!("/api/v1/verify/t/{}", v.token)).await;
    assert_eq!(meta.status, 200, "{}", meta.text());
    let body = meta.json();
    assert_eq!(body["digitally_sealed"], true);
    assert_eq!(body["seal_level"], "pades-b-b");
    assert_eq!(body["sealed_hash"], json!(sha(&sealed)));
    assert_eq!(body["document_hash"], json!(sha(&master)));
    let view = app
        .get("", &format!("/api/v1/verify/t/{}/pdf", v.token))
        .await;
    assert_eq!(view.status, 200, "{}", view.text());
    assert_eq!(view.body, sealed);
    let dl = app
        .get(
            "",
            &format!("/api/v1/verify/t/{}/pdf?download=true", v.token),
        )
        .await;
    assert_eq!(dl.status, 200);
    assert_eq!(dl.body, sealed);
    // staff download
    let staff = app
        .get(&sa.token, &format!("/api/v1/verify/{}/pdf", v.id))
        .await;
    assert_eq!(staff.status, 200, "{}", staff.text());
    assert_eq!(staff.body, sealed);
    // email by the salesperson: the same sealed copy, sealed once (the stored copy is reused)
    send(
        app,
        &bdr.token,
        "quotation",
        qid,
        json!({"channel": "email", "to": [email]}),
    )
    .await;
    assert_eq!(last_attachment(), sealed);
    let again = active_verification(app, qid).await;
    assert_eq!(again.id, v.id);
    assert_eq!(
        (again.sealed_key, again.sealed_hash),
        (v.sealed_key, v.sealed_hash)
    );

    // a customer can still sign the sealed copy without breaking ACME's seal
    let signed = pdf_sign::_append_approval_signature(&sealed, "CustomerSignature")
        .await
        .unwrap();
    assert_sealed(&signed);
}

#[tokio::test]
async fn setting_off_sends_the_master_and_a_legacy_copy_is_sealed_on_its_next_send() {
    let (app, _g) = setup().await;
    let sa = owner(app).await;
    let (iid, email) = invoice(app).await;
    app.set_setting("documents.seal.outbound", "false").await;
    send(
        app,
        &sa.token,
        "invoice",
        iid,
        json!({"channel": "email", "to": [email]}),
    )
    .await;
    app.set_setting("documents.seal.outbound", "true").await;
    let master = anchors_pdf();
    assert_eq!(
        last_attachment(),
        master,
        "setting off: the unsealed master"
    );
    let v = active_verification(app, iid).await;
    assert!(
        v.sealed_key.is_none() && !v.sealed,
        "nothing sealed while off"
    );
    let meta = app
        .get("", &format!("/api/v1/verify/t/{}", v.token))
        .await
        .json();
    assert_eq!(meta["digitally_sealed"], false);
    assert!(meta["seal_level"].is_null() && meta["sealed_hash"].is_null());

    // like the documents issued before the seal: sealed on the next send while still valid (WhatsApp here)
    send(
        app,
        &sa.token,
        "invoice",
        iid,
        json!({"channel": "whatsapp", "to": ["+81 80 1111 2222"], "force": true}),
    )
    .await;
    let wa = WA_SENT.lock().unwrap().clone();
    assert_eq!(wa.len(), 1);
    let sent =
        granary_compat::crypto::pyb64::b64decode_str(wa[0]["data"].as_str().unwrap()).unwrap();
    assert_sealed(&sent);
    let after = active_verification(app, iid).await;
    assert_eq!(after.id, v.id, "the same verification, not a new issue");
    assert!(after.sealed && after.level.as_deref() == Some("pades-b-b"));
    assert_eq!(stored(after.sealed_key.as_deref().unwrap()).unwrap(), sent);
    assert_eq!(after.sealed_hash.as_deref(), Some(sha(&sent).as_str()));
}

#[tokio::test]
async fn a_failed_seal_sends_the_master_and_alerts_the_owner_once() {
    let (app, _g) = setup().await;
    let sa = owner(app).await;
    let junk = b"%PDF-1.4 not a real document".to_vec();
    *RENDER.lock().unwrap() = Some(junk.clone());
    let (iid, email) = invoice(app).await;
    send(
        app,
        &sa.token,
        "invoice",
        iid,
        json!({"channel": "email", "to": [email.clone()]}),
    )
    .await;
    assert_eq!(last_attachment(), junk, "sending is never blocked");
    let v = active_verification(app, iid).await;
    assert!(v.sealed_key.is_none() && !v.sealed);
    let key = format!("documents.seal:{}", v.id);
    let alerts = |key: String| async move {
        let mut db = app.db().await;
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT title, link FROM notifications WHERE type = 'nary_error' AND entity_id = $1",
        )
        .bind(&key)
        .fetch_all(&mut db)
        .await
        .unwrap();
        rows
    };
    let first = alerts(key.clone()).await;
    assert!(!first.is_empty(), "the Owner is alerted");
    assert_eq!(first[0].0, "A document went out without its digital seal");
    assert_eq!(
        first[0].1.as_deref(),
        Some(format!("/finance/invoices/{iid}").as_str())
    );

    // a second send tries again, fails again, and does not alert again
    send(
        app,
        &sa.token,
        "invoice",
        iid,
        json!({"channel": "email", "to": [email], "force": true}),
    )
    .await;
    assert_eq!(last_attachment(), junk);
    assert_eq!(
        alerts(key).await.len(),
        first.len(),
        "one alert per document"
    );
    *RENDER.lock().unwrap() = None;
}
