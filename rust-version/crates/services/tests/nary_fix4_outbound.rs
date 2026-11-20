//! Nary fix batch 4 (agent D2): the rate card, cost sheets, product files and files on customer messages.
//!
//! - get_rate_card / get_cost_sheet: internal, sales:view_costs; the man-day cost, the salary behind it, the
//!   proposal.cost.* settings; a proposal's cost sheet (labour, non-labour, summary).
//! - find_product_files: product files on catalog items (attachments 'catalog_product'), Knowledge Base files,
//!   customer-safe AI knowledge documents; only safe ones; the catalog read rule on product files.
//! - propose_customer_message attach on WhatsApp: the Contoso case (a quotation by its number and the
//!   "Acme - Laptop Pro 14 - Datasheet.pdf" product file on one card); the card lists both with size and
//!   approval and stays a card at Act; the confirm sends the text, then each file as a document (the
//!   quotation with its number as the caption), stores them in the chat, and marks the quotation sent.
//! - An unapproved quotation: the card says so; the confirm sends nothing (409).
//! - Email: a file goes as an email attachment with the text.
//! - Caps (nary.attach.max_files, nary.attach.max_total_mb) and a file the person may not open.

mod nary_n2_common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog;
use granary_services::email::{self as email_service, Seams, SendCall, SmtpConfig};
use granary_services::nary::executor;
use granary_services::storage::{self, ObjectStore, StoreError};
use granary_services::verification::delivery::seams as delivery_seams;
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::ResponseTemplate;

const TAG: &str = "nary_fix4_outbound";
const DATASHEET: &str = "Acme - Laptop Pro 14 - Datasheet.pdf";

// ─── fixtures ────────────────────────────────────────────────────────────────

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

fn store() -> Arc<MemStore> {
    let s = Arc::new(MemStore::default());
    storage::set_store(Some(s.clone()));
    s
}

/// Verified PDFs without the PDF service.
fn fake_pdfs() {
    delivery_seams::reset();
    delivery_seams::set_verified_pdf(Some(Arc::new(|_db, _t, _e, _u| {
        Box::pin(async { Ok((None, b"%PDF-1.4 quotation".to_vec())) })
    })));
}

/// The WhatsApp service's /send-media: recorded, answered with a message id.
fn media_route() -> Arc<Mutex<Vec<Value>>> {
    let got: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(vec![]));
    let rec = got.clone();
    route("POST", "/send-media", move |req| {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let to = body["to"].as_str().unwrap_or("").to_string();
        rec.lock().unwrap().push(body);
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
            "messageId": format!("true_{to}_{}", Uuid::new_v4().simple().to_string()[..16].to_uppercase())}))
    });
    got
}

type Mails = Arc<Mutex<Vec<SendCall>>>;

fn smtp() -> Mails {
    let sent: Mails = Arc::new(Mutex::new(vec![]));
    let rec = sent.clone();
    email_service::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call: SendCall| {
            rec.lock().unwrap().push(call);
            Box::pin(async { Ok((true, None)) })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: "u".into(),
                    password: "p".into(),
                    from_name: "Your Company Ltd".into(),
                    from_email: "hello@example.com".into(),
                    tls: true,
                })
            })
        })),
    }));
    sent
}

fn cleanup() {
    storage::set_store(None);
    delivery_seams::reset();
    email_service::set_global_seams(None);
}

async fn the_owner(pool: &PgPool) -> AuthUser {
    let id = owner(pool).await;
    let mut c = pool.acquire().await.unwrap();
    auth_user(&mut c, id).await
}

/// A person with exactly these permissions (not a superadmin).
async fn person(pool: &PgPool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password, password_changed_at) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal', true, false, now())",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("d2.person")))
    .bind(u("D2 Person"))
    .execute(pool)
    .await
    .unwrap();
    let mut c = pool.acquire().await.unwrap();
    let mut user = auth_user(&mut c, id).await;
    user.perms = perms.iter().map(|p| p.to_string()).collect();
    user
}

async fn tool(pool: &PgPool, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut c = pool.acquire().await.unwrap();
    catalog::run_tool(&mut c, user, name, args).await
}

/// A chat where the customer has written.
async fn customer_chat(pool: &PgPool) -> String {
    let cid = chat_id();
    ingest(
        pool,
        &payload(
            &cid,
            "Hello, could I have a quotation and the datasheet?",
            false,
            None,
            None,
            json!({}),
        ),
        "live",
    )
    .await;
    cid
}

/// A draft quotation with a catalog line (or a custom line when `custom`); (id, number).
async fn quotation(pool: &PgPool, custom: bool) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let number = u("QUO-2026-10");
    let today = chrono::Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, total_quantity, \
         validity_days, procurement_type, customer_po_accepted_verbally, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, 20000000, 20000000, '{\"text\": \"NET30\"}', 0, 0, 0, '[]', 0, 0, \
         14, 'standard', false, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(today)
    .bind(today + chrono::Duration::days(14))
    .execute(pool)
    .await
    .unwrap();
    let product: Option<Uuid> = if custom {
        None
    } else {
        sqlx::query_scalar("SELECT id FROM product_catalog WHERE is_deleted = false LIMIT 1")
            .fetch_optional(pool)
            .await
            .unwrap()
    };
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, product_id, name, quantity, unit_price, currency, \
         discount_percentage, discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, $3, 'Line', 2, 10000000, 'JPY', 0, 'none', 20000000, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(product)
    .execute(pool)
    .await
    .unwrap();
    (id, number)
}

/// A catalog item "Laptop Pro 14" with its datasheet as a product file; (product id, file id).
async fn datasheet(pool: &PgPool, store: &MemStore, bytes: usize) -> (Uuid, Uuid) {
    let pid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, family, product_type, sale_model, price_metric, pricing_mode, \
         is_whitelabel, is_active, is_orderable, is_deleted) \
         VALUES ($1, $2, 'Laptop Pro 14', 'Laptops', 'hardware', 'one_time', 'unit', 'list', false, true, true, false)",
    )
    .bind(pid)
    .bind(u("LAPTOP-S25"))
    .execute(pool)
    .await
    .unwrap();
    let fid = Uuid::new_v4();
    let key = format!("attachments/catalog_product/{pid}/{fid}/{DATASHEET}");
    store
        .blobs
        .lock()
        .unwrap()
        .insert(key.clone(), vec![b'D'; bytes]);
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, \
         file_type) VALUES ($1, 'catalog_product', $2, $3, $4, 'application/pdf', $5, 'http://x', 'file')",
    )
    .bind(fid)
    .bind(pid.to_string())
    .bind(DATASHEET)
    .bind(bytes as i32)
    .bind(&key)
    .execute(pool)
    .await
    .unwrap();
    (pid, fid)
}

async fn quotation_status(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ─── the rate card and cost sheets ───────────────────────────────────────────

#[test]
fn test_rate_card_gives_the_man_day_cost_the_salary_and_the_settings() {
    run(TAG, |pool| async move {
        let role = u("Pytest Engineer");
        sqlx::query(
            "INSERT INTO nary_rate_card (id, role, seniority, cost_base, source, valid_from, status, proposed_by_nary) \
             VALUES ($1, $2, 'mid', 514000, 'https://salary.example (gross 9000000/month; Tokyo)', '2026-10-01', \
             'active', true)",
        )
        .bind(Uuid::new_v4())
        .bind(&role)
        .execute(&pool)
        .await
        .unwrap();
        let owner = the_owner(&pool).await;
        let out = tool(&pool, &owner, "get_rate_card", json!({})).await;
        let row = out["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == role.as_str())
            .unwrap_or_else(|| panic!("{out}"))
            .clone();
        assert_eq!(row["man_day_cost_base"], "514000");
        assert_eq!(row["monthly_salary_base"], "9000000");
        // 514000 x 21 / 1.2 = 8995000
        assert_eq!(row["monthly_equivalent_base"], "8995000");
        assert!(row["source"].as_str().unwrap().contains("salary.example"));
        assert_eq!(out["settings"]["proposal.cost.days_per_month"], 21);
        assert_eq!(out["settings"]["proposal.cost.load_factor"], "1.2");
        assert_eq!(out["settings"]["proposal.cost.contingency_pct"], "15");

        let seller = person(&pool, &["sales:view_catalog", "nary:approve"]).await;
        let denied = tool(&pool, &seller, "get_rate_card", json!({})).await;
        assert!(
            denied["error"].as_str().unwrap().contains("permission"),
            "{denied}"
        );
        let costs = person(&pool, &["sales:view_costs"]).await;
        assert!(tool(&pool, &costs, "get_rate_card", json!({})).await["roles"].is_array());
    });
}

#[test]
fn test_cost_sheet_of_a_proposal() {
    run(TAG, |pool| async move {
        let doc = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO sales_documents (id, doc_type, number, revision, title, language, status, validity_days, \
             cover, is_deleted) VALUES ($1, 'solution_proposal', $2, 1, 'Traffic counting', 'en', 'draft', 30, '{}', false)",
        )
        .bind(doc)
        .bind(u("SP"))
        .execute(&pool)
        .await
        .unwrap();
        let owner = the_owner(&pool).await;
        let none = tool(
            &pool,
            &owner,
            "get_cost_sheet",
            json!({"document_id": doc.to_string()}),
        )
        .await;
        assert!(
            none["sheet"].is_null() && none["note"].as_str().unwrap().contains("no cost sheet"),
            "{none}"
        );
        let content = json!({
            "labour": {"workstreams": [{"name": "Setup", "lines": [{"role": "Engineer", "seniority": "mid",
                "low": 2, "likely": 3, "high": 4, "day_rate": "500000", "cost_likely": "1500000.00"}]}],
                "totals": {"low": "1000000.00", "likely": "1500000.00", "high": "2000000.00"}, "missing_rates": []},
            "non_labour": {"items": [{"item": "Laptop", "kind": "hardware", "quantity": 1, "cost_base": "9000000.00"}],
                "total": "9000000.00", "to_confirm": []},
            "contingency_pct": "15",
            "summary": {"likely": {"cost": "12075000.00", "price_at_target": "20125000.00"}},
            "category": "project_services", "assumptions": ["Site ready"], "currency": "JPY"});
        sqlx::query(
            "INSERT INTO nary_cost_sheets (id, document_id, content, currency, status) VALUES ($1, $2, $3, 'JPY', 'draft')",
        )
        .bind(Uuid::new_v4())
        .bind(doc)
        .bind(&content)
        .execute(&pool)
        .await
        .unwrap();
        let out = tool(
            &pool,
            &owner,
            "get_cost_sheet",
            json!({"document_id": doc.to_string()}),
        )
        .await;
        assert_eq!(out["title"], "Traffic counting");
        assert_eq!(out["labour"]["workstreams"][0]["lines"][0]["likely"], 3);
        assert_eq!(out["non_labour"]["total"], "9000000.00");
        assert_eq!(out["summary"]["likely"]["cost"], "12075000.00");
        assert_eq!(out["assumptions"], json!(["Site ready"]));
        let missing = tool(
            &pool,
            &owner,
            "get_cost_sheet",
            json!({"document_id": "NOPE-1"}),
        )
        .await;
        assert!(
            missing["error"].as_str().unwrap().contains("No proposal"),
            "{missing}"
        );
        let seller = person(&pool, &["sales:view_catalog"]).await;
        let denied = tool(
            &pool,
            &seller,
            "get_cost_sheet",
            json!({"document_id": doc.to_string()}),
        )
        .await;
        assert!(denied["error"].is_string() && denied.get("labour").is_none());
    });
}

// ─── product files ───────────────────────────────────────────────────────────

#[test]
fn test_find_product_files_searches_products_kb_and_safe_knowledge() {
    run(TAG, |pool| async move {
        let s = store();
        let (_, fid) = datasheet(&pool, &s, 2048).await;
        // a customer-safe AI knowledge document and one that is not safe
        for (title, safe) in [
            ("Laptop Pro 14 brochure", true),
            ("Laptop Pro 14 internal costs", false),
        ] {
            sqlx::query(
                "INSERT INTO ai_knowledge_docs (id, title, source_type, original_filename, original_object_key, \
                 original_size, status, is_active, customer_safe, chunk_count, use_count) \
                 VALUES ($1, $2, 'pdf', $3, 'ai-knowledge/x.pdf', 999, 'ready', true, $4, 0, 0)",
            )
            .bind(Uuid::new_v4())
            .bind(title)
            .bind(format!("{title}.pdf"))
            .bind(safe)
            .execute(&pool)
            .await
            .unwrap();
        }
        let owner = the_owner(&pool).await;
        let out = tool(
            &pool,
            &owner,
            "find_product_files",
            json!({"query": "Laptop Pro 14 datasheet"}),
        )
        .await;
        let files = out["files"].as_array().unwrap();
        // (other tests of this file add datasheets of their own to the same database)
        assert!(files.iter().all(|f| f["name"] == DATASHEET), "{out}");
        let f = files
            .iter()
            .find(|f| f["file_id"] == fid.to_string())
            .unwrap_or_else(|| panic!("{out}"));
        assert_eq!(f["size"], "2 KB");
        assert_eq!(f["source"], "product file");
        assert_eq!(f["product"]["name"], "Laptop Pro 14");
        let wide = tool(
            &pool,
            &owner,
            "find_product_files",
            json!({"query": "Laptop Pro 14"}),
        )
        .await;
        let names: Vec<&str> = wide["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&DATASHEET) && names.contains(&"Laptop Pro 14 brochure.pdf"),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("internal costs")),
            "{names:?}"
        );
        let nothing = tool(
            &pool,
            &owner,
            "find_product_files",
            json!({"query": "zzqx"}),
        )
        .await;
        assert_eq!(nothing["count"], 0);
        assert!(nothing["note"].as_str().unwrap().contains("Product files"));

        // product files follow the catalog's read rule
        let mut c = pool.acquire().await.unwrap();
        let pid: String = sqlx::query_scalar("SELECT entity_id FROM attachments WHERE id = $1")
            .bind(fid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
        let outsider = person(&pool, &["crm:view_own"]).await;
        assert!(granary_services::attachments_access::assert_can_access(
            &mut c,
            &outsider,
            "catalog_product",
            &pid
        )
        .await
        .is_err());
        let viewer = person(&pool, &["sales:view_catalog"]).await;
        assert!(granary_services::attachments_access::assert_can_access(
            &mut c,
            &viewer,
            "catalog_product",
            &pid
        )
        .await
        .is_ok());
        drop(c);
        cleanup();
    });
}

// ─── files on customer messages ──────────────────────────────────────────────

#[test]
fn test_initech_case_quotation_and_datasheet_on_one_whatsapp_card() {
    run(TAG, |pool| async move {
        let s = store();
        fake_pdfs();
        let media = media_route();
        working_hours(true);
        let owner = the_owner(&pool).await;
        let cid = customer_chat(&pool).await;
        let (qid, number) = quotation(&pool, false).await;
        let (_, fid) = datasheet(&pool, &s, 300 * 1024).await;
        // Act and tell on WhatsApp replies: a message with files still stays a card
        let old = set(&pool, &[("nary.shadow_mode", "false")]).await;
        let levels: String = sqlx::query_scalar(
            "SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut lv: Value = serde_json::from_str(&levels).unwrap();
        lv["whatsapp.reply.send"] = json!(2);
        let old2 = set(&pool, &[("nary.autonomy.levels", &lv.to_string())]).await;

        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "whatsapp", "chat_id": cid,
                   "text": "Good afternoon Mr Smith, here are the quotation and the Laptop Pro 14 datasheet.",
                   "attach": [{"type": "quotation", "id": number}, {"type": "file", "id": fid.to_string()}]}),
        )
        .await;
        let p = out["proposal"].clone();
        assert!(p.is_object(), "{out}");
        assert!(
            p.get("auto").is_none(),
            "a message with files is a card: {out}"
        );
        let labels: Vec<String> = p["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap().to_string())
            .collect();
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert!(
            labels[0].starts_with("Quotation ") && labels[0].contains("PDF"),
            "{labels:?}"
        );
        assert!(labels[0].contains("approved"), "{labels:?}");
        assert_eq!(labels[1], format!("{DATASHEET} (300 KB, PDF)"));
        assert_eq!(
            p["attach"][0],
            json!({"type": "quotation", "id": qid.to_string()})
        );
        assert_eq!(p["attach"][1]["type"], "file");
        let shown = granary_services::nary::staff_chat::describe(&p);
        assert!(shown.contains(DATASHEET), "{shown}");
        assert!(
            sent().is_empty() && media.lock().unwrap().is_empty(),
            "nothing goes at the proposal"
        );

        // the confirm (as the web card and the WhatsApp "1" do: /nary/send -> send_on_instruction)
        let mut c = pool.acquire().await.unwrap();
        let row = executor::send_on_instruction(
            &mut c,
            &owner,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": p["text"], "attach": p["attach"]}),
            None,
        )
        .await
        .unwrap();
        drop(c);
        assert_eq!(row.status, "applied", "{:?}", row.result);
        let texts = sent();
        assert_eq!(texts.len(), 1);
        assert!(texts[0]["message"]
            .as_str()
            .unwrap()
            .contains("here are the quotation"));
        let files = media.lock().unwrap().clone();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["mimetype"], "application/pdf");
        assert!(files[0]["caption"]
            .as_str()
            .unwrap()
            .starts_with("Quotation "));
        assert!(files[0]["filename"].as_str().unwrap().ends_with(".pdf"));
        assert_eq!(files[1]["filename"], DATASHEET);
        assert_eq!(files[1]["caption"], "");
        assert_eq!(
            files[1]["data"].as_str().unwrap().len(),
            (300 * 1024usize).div_ceil(3) * 4
        );
        // both are in the chat as Nary's documents
        let docs: Vec<(String, Option<String>, bool)> = sqlx::query_as(
            "SELECT type, media_name, sent_by_nary FROM wa_messages WHERE chat_id = $1 AND type = 'document' \
             ORDER BY created_at",
        )
        .bind(&cid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(docs.len(), 2, "{docs:?}");
        assert!(docs.iter().all(|d| d.2));
        assert_eq!(docs[1].1.as_deref(), Some(DATASHEET));
        // the quotation is sent, as the ERP's Send leaves it, and its follow-up cadence starts
        assert_eq!(quotation_status(&pool, qid).await, "sent");
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM domain_events WHERE type = 'quotation.sent' AND entity_id = $1",
        )
        .bind(qid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 1);
        restore(&pool, old2).await;
        restore(&pool, old).await;
        unroute("POST", "/send-media");
        cleanup();
    });
}

#[test]
fn test_an_unapproved_quotation_goes_for_approval_and_nothing_is_sent() {
    run(TAG, |pool| async move {
        let _s = store();
        fake_pdfs();
        let media = media_route();
        working_hours(true);
        let _owner = the_owner(&pool).await;
        let seller = person(
            &pool,
            &[
                "nary:approve",
                "sales:send_quotation",
                "approvals:view",
                "crm:view_all",
                "crm:send_whatsapp",
            ],
        )
        .await;
        let cid = customer_chat(&pool).await;
        let (qid, _) = quotation(&pool, true).await; // a custom line: the Owner must approve
        let out = tool(
            &pool,
            &seller,
            "propose_customer_message",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Here is the quotation, Mr.",
                   "attach": [{"type": "quotation", "id": qid.to_string()}]}),
        )
        .await;
        let label = out["proposal"]["attachments"][0]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(label.contains("not approved yet"), "{out}");
        let mut c = pool.acquire().await.unwrap();
        let e = executor::send_on_instruction(
            &mut c,
            &seller,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Here is the quotation, Mr.",
                   "attach": out["proposal"]["attach"]}),
            None,
        )
        .await
        .unwrap_err();
        drop(c);
        assert_eq!(e.status().as_u16(), 409, "{e:?}");
        assert!(format!("{e:?}").contains("went for approval"), "{e:?}");
        assert!(sent().is_empty() && media.lock().unwrap().is_empty());
        assert_eq!(quotation_status(&pool, qid).await, "draft");
        unroute("POST", "/send-media");
        cleanup();
    });
}

#[test]
fn test_an_email_carries_a_product_file() {
    run(TAG, |pool| async move {
        let s = store();
        let mails = smtp();
        let owner = the_owner(&pool).await;
        let (_, fid) = datasheet(&pool, &s, 1000).await;
        let to = format!("{}@initech.example", u("adam"));
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "email", "to": to, "subject": "Datasheet Laptop Pro 14",
                   "text": "Dear Mr Adam,\n\nPlease find the datasheet attached.\n\nBest regards,",
                   "attach": [{"type": "file", "id": fid.to_string()}]}),
        )
        .await;
        let p = out["proposal"].clone();
        assert_eq!(
            p["attachments"][0],
            format!("{DATASHEET} (1000 B, PDF)"),
            "{out}"
        );
        let mut c = pool.acquire().await.unwrap();
        let row = executor::send_on_instruction(
            &mut c,
            &owner,
            "email.reply.send",
            json!({"channel": "email", "to": to, "subject": p["subject"], "text": p["text"], "attach": p["attach"]}),
            None,
        )
        .await
        .unwrap();
        drop(c);
        assert_eq!(row.status, "applied", "{:?}", row.result);
        let mails = mails.lock().unwrap().clone();
        assert_eq!(mails.len(), 1);
        let files = mails[0].attachments.clone().unwrap_or_default();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, DATASHEET);
        assert_eq!(files[0].content.len(), 1000);
        cleanup();
    });
}

#[test]
fn test_caps_and_files_the_person_may_not_open() {
    run(TAG, |pool| async move {
        let s = store();
        working_hours(true);
        let owner = the_owner(&pool).await;
        let cid = customer_chat(&pool).await;
        let (_, fid) = datasheet(&pool, &s, 4096).await;
        let ask = |attach: Value| json!({"channel": "whatsapp", "chat_id": cid, "text": "Here is the file, Mr.", "attach": attach});
        let old = set(
            &pool,
            &[
                ("nary.attach.max_files", "1"),
                ("nary.attach.max_total_mb", "0"),
            ],
        )
        .await;
        let two = json!([{"type": "file", "id": fid.to_string()}, {"type": "file", "id": fid.to_string()}]);
        let out = tool(&pool, &owner, "propose_customer_message", ask(two)).await;
        assert!(
            out["error"]
                .as_str()
                .unwrap()
                .contains("nary.attach.max_files"),
            "{out}"
        );
        let one = json!([{"type": "file", "id": fid.to_string()}]);
        let out = tool(&pool, &owner, "propose_customer_message", ask(one.clone())).await;
        assert!(
            out["error"]
                .as_str()
                .unwrap()
                .contains("nary.attach.max_total_mb"),
            "{out}"
        );
        restore(&pool, old).await;

        // a deal's file the person can't see (owner scoping) is refused
        let other = person(&pool, &[]).await;
        let deal = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO deals (id, title, stage, value, currency, probability, assigned_to_id, is_deleted) \
             VALUES ($1, 'Pytest deal', 'discovery', 0, 'JPY', 10, $2, false)",
        )
        .bind(deal)
        .bind(other.id)
        .execute(&pool)
        .await
        .unwrap();
        let att = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, \
             file_type) VALUES ($1, 'crm_deal', $2, 'site-survey.pdf', 10, 'application/pdf', 'k', 'http://x', 'file')",
        )
        .bind(att)
        .bind(deal.to_string())
        .execute(&pool)
        .await
        .unwrap();
        let seller = person(&pool, &["nary:approve", "crm:view_own"]).await;
        let out = tool(
            &pool,
            &seller,
            "propose_customer_message",
            ask(json!([{"type": "file", "id": att.to_string()}])),
        )
        .await;
        assert!(
            out["error"]
                .as_str()
                .unwrap()
                .contains("can't send site-survey.pdf"),
            "{out}"
        );
        let bogus = tool(
            &pool,
            &owner,
            "propose_customer_message",
            ask(json!([{"type": "file", "id": Uuid::new_v4().to_string()}])),
        )
        .await;
        assert!(
            bogus["error"]
                .as_str()
                .unwrap()
                .starts_with("No file with id"),
            "{bogus}"
        );
        cleanup();
    });
}

// ─── the tools themselves ────────────────────────────────────────────────────

#[test]
fn test_the_new_tools_are_listed_and_their_schemas_pass_the_anthropic_rules() {
    use granary_services::ai::{mcp_access, schema};
    for name in [
        "get_rate_card",
        "get_cost_sheet",
        "find_product_files",
        "propose_customer_message",
    ] {
        let t = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} missing"));
        let sent = schema::for_anthropic(&t.parameters);
        assert!(
            schema::violations(&sent).is_empty(),
            "{name}: {:?}",
            schema::violations(&sent)
        );
        assert!(!t.description.contains('\u{2014}'), "{name} has an em dash");
    }
    for name in ["get_rate_card", "get_cost_sheet", "find_product_files"] {
        assert!(
            !mcp_access::PROPOSAL_TOOLS.contains(&name),
            "{name} is a read tool"
        );
    }
    let pcm = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "propose_customer_message")
        .unwrap();
    assert_eq!(
        pcm.parameters["properties"]["attach"]["items"]["properties"]["type"]["enum"],
        json!(["quotation", "invoice", "sales_document", "file"])
    );
    let costs = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "get_rate_card")
        .unwrap();
    assert_eq!(costs.required_any, vec!["sales:view_costs".to_string()]);
}
