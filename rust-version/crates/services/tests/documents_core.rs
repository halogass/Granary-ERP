//! Port of the documents parts of backend/tests/test_nary_p4_documents.py (the document, the revision, the
//! render, costs kept out of the PDF), plus white-box checks of services/documents (errors, links, the
//! fingerprint read back from the database, the MinIO image store).
//!
//! The Python `store` fixture replaces MinIO by a dict; here MinIO is a wiremock server (path-style S3),
//! started once per test binary because `MINIO_ENDPOINT` is read once.
//! Not ported here: test_sending_a_proposal_attaches_its_quotations (services/verification/delivery, its
//! own agent), the cost sheet, guards and Nary tests (services/nary, wave 3).

mod support;

use chrono::{Duration, Local};
use granary_core::auth::{load_active_user, AuthUser};
use granary_core::errors::ApiError;
use granary_db::rows::crm::Deal;
use granary_db::rows::documents::SalesDocument;
use granary_services::documents::{assets, core_, render};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static MINIO: tokio::sync::OnceCell<MockServer> = tokio::sync::OnceCell::const_new();

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

async fn minio() -> &'static MockServer {
    MINIO
        .get_or_init(|| async {
            let server = MockServer::start().await;
            std::env::set_var("MINIO_ENDPOINT", server.address().to_string());
            server
        })
        .await
}

async fn pool() -> PgPool {
    minio().await;
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("s10_documents").await;
            pool.close().await;
            std::env::var("DATABASE_URL").unwrap()
        })
        .await;
    PgPool::connect(url).await.expect("connect")
}

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

fn detail(e: &ApiError) -> (u16, String) {
    match e {
        ApiError::Http { status, detail, .. } => (
            status.as_u16(),
            detail.as_str().unwrap_or_default().to_string(),
        ),
        other => panic!("not an HTTP error: {other:?}"),
    }
}

async fn owner(db: &mut PgConnection) -> AuthUser {
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let mut user = load_active_user(db, id).await.unwrap().unwrap();
    user.perms.insert("sales:create_quotation".into());
    user
}

struct Customer {
    contact_id: Uuid,
    org_id: Uuid,
}

async fn customer(db: &mut PgConnection) -> Customer {
    let org_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(org_id)
    .bind(u("Pytest Corp"))
    .execute(&mut *db)
    .await
    .unwrap();
    let contact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, organization_id, is_deleted, custom) \
         VALUES ($1, 'John', $2, $3, false, '{}')",
    )
    .bind(contact_id)
    .bind(u("Pytest"))
    .bind(org_id)
    .execute(&mut *db)
    .await
    .unwrap();
    Customer { contact_id, org_id }
}

async fn deal_for(db: &mut PgConnection, c: &Customer) -> Deal {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, contact_id, organization_id, stage, currency, value, is_deleted, custom) \
         VALUES ($1, $2, $3, $4, 'proposal', 'JPY', 100000000, false, '{}')",
    )
    .bind(id)
    .bind(u("Pytest deal"))
    .bind(c.contact_id)
    .bind(c.org_id)
    .execute(&mut *db)
    .await
    .unwrap();
    let cols = Deal::COLUMNS.join(", ");
    sqlx::query_as(&format!("SELECT {cols} FROM deals WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// `quotation_for(deal_id, cust, product, status=...)`: one line, 2 x 15,000,000 JPY.
async fn quotation_for(db: &mut PgConnection, deal: &Deal, c: &Customer, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    let amount = 30_000_000i64;
    let today = Local::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, deal_id, customer_contact_id, customer_org_id, \
         issue_date, valid_until, subtotal, total, payment_terms, discount_value, discount_amount, tax_amount, tax_lines, \
         rounded_total, total_quantity, validity_days, procurement_type, customer_po_accepted_verbally, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', $3, $4, $5, $6, $7, $8, $9, $9, $10, 0, 0, 0, '[]', 0, 0, 30, 'standard', false, false)",
    )
    .bind(id)
    .bind(u("PYTEST-Q"))
    .bind(status)
    .bind(deal.id)
    .bind(c.contact_id)
    .bind(c.org_id)
    .bind(today)
    .bind(today + Duration::days(30))
    .bind(rust_decimal::Decimal::from(amount))
    .bind(json!({"text": "50% down payment", "schedule": [
        {"label": "Down payment", "percentage": 50.0, "due_trigger": "on_order"},
        {"label": "On delivery", "percentage": 50.0, "due_trigger": "on_milestone"}]}))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, name, quantity, unit_price, currency, discount_percentage, \
         discount_type, amount, list_unit_price, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, 'Pytest Laptop Server 25', 2, 15000000, 'JPY', 0, 'none', $3, 15000000, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(rust_decimal::Decimal::from(amount))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn get_doc(db: &mut PgConnection, id: Uuid) -> SalesDocument {
    let cols = [
        "doc_type",
        "number",
        "full_number",
        "revision",
        "revision_group",
        "previous_version_id",
        "revision_reason",
        "title",
        "subtitle",
        "language",
        "deal_id",
        "organization_id",
        "contact_id",
        "status",
        "validity_days",
        "classification",
        "cover",
        "issue_date",
        "created_by_id",
        "sent_at",
        "origin",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ]
    .join(", ");
    sqlx::query_as(&format!("SELECT {cols} FROM sales_documents WHERE id = $1"))
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

/// `_doc(cust, deal_id, quotation=True)`.
async fn make_doc(pool: &PgPool, quotation: bool) -> (Uuid, AuthUser, Vec<Uuid>, Deal) {
    let mut tx = pool.begin().await.unwrap();
    let me = owner(&mut tx).await;
    let cust = customer(&mut tx).await;
    let deal = deal_for(&mut tx, &cust).await;
    let qids = if quotation {
        vec![quotation_for(&mut tx, &deal, &cust, "draft").await]
    } else {
        vec![]
    };
    let doc = core_::create(
        &mut tx,
        core_::CreateArgs {
            user_id: Some(me.id),
            title: "AI Construction Safety Monitoring".into(),
            subtitle: Some("15 laptops".into()),
            language: "en".into(),
            deal: Some(&deal),
            quotation_ids: qids.clone(),
            cover: Some(json!({"prepared_for": "Pytest Corp", "attention": "John"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (doc.id, me, qids, deal)
}

// ─── The document ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_a_new_proposal_has_the_template_sections_and_its_number() {
    let pool = pool().await;
    let (doc_id, _, _, deal) = make_doc(&pool, true).await;
    let mut db = pool.acquire().await.unwrap();
    let doc = get_doc(&mut db, doc_id).await;
    let blocks = core_::blocks_of(&mut db, doc_id).await.unwrap();
    let template = core_::template(&mut db).await.unwrap();
    assert!(doc.number.starts_with("PRP"));
    assert_eq!(
        doc.full_number.as_deref(),
        Some(format!("{} Rev. 1", doc.number).as_str())
    );
    assert_eq!(doc.status, "draft");
    assert_eq!(template[0]["key"], "executive_summary");
    let sections: std::collections::BTreeSet<String> =
        blocks.iter().map(|b| b.section.clone()).collect();
    let keys: std::collections::BTreeSet<String> = template
        .iter()
        .map(|s| s["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(sections, keys);
    assert!(
        blocks.iter().any(|b| b.block_type == "commercial")
            && blocks.iter().any(|b| b.block_type == "stamp")
    );
    // create() fills the rest as Python does
    assert_eq!(doc.revision_group.as_deref(), Some(doc.number.as_str()));
    assert_eq!(doc.organization_id, deal.organization_id);
    assert_eq!(doc.contact_id, deal.contact_id);
    assert_eq!(doc.origin.as_deref(), Some("manual"));
    assert_eq!(doc.classification.as_deref(), Some("Confidential"));
    let positions: Vec<i32> = blocks.iter().map(|b| b.position).collect();
    assert_eq!(positions, (0..blocks.len() as i32).collect::<Vec<_>>());
}

#[tokio::test]
async fn test_an_edit_invalidates_the_approval() {
    use granary_services::approvals::engine;
    let pool = pool().await;
    let (doc_id, me, _, _) = make_doc(&pool, true).await;
    let mut tx = pool.begin().await.unwrap();
    // 1 Nov 2026: a proposal needs its required figures before approval; this one is about the fingerprint
    sqlx::query(
        "UPDATE system_settings SET value = '[]' WHERE key = 'proposal.visuals.default_figures'",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    let req = engine::submit(
        &mut tx,
        "sales_document",
        &doc_id.to_string(),
        &me,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    if req.status != "approved" {
        let mut tx = pool.begin().await.unwrap();
        engine::act(
            &mut tx,
            &req.id.to_string(),
            &me,
            "approved",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    assert!(engine::is_approved(&mut tx, "sales_document", doc_id)
        .await
        .unwrap());
    let blocks = core_::blocks_of(&mut tx, doc_id).await.unwrap();
    let mut edited: Vec<Value> = blocks
        .iter()
        .map(|b| json!({"section": b.section, "block_type": b.block_type, "data": b.data}))
        .collect();
    edited[0]["data"] = json!({"md": "A changed executive summary."});
    sqlx::query("UPDATE sales_documents SET status = 'review' WHERE id = $1")
        .bind(doc_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let doc = get_doc(&mut tx, doc_id).await;
    core_::replace_blocks(&mut tx, &doc, &edited).await.unwrap();
    tx.commit().await.unwrap();
    let mut db = pool.acquire().await.unwrap();
    assert!(!engine::is_approved(&mut db, "sales_document", doc_id)
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/sales_hooks.rs::test_a_revision_supersedes_the_proposal_and_its_sent_quotations (wave 4)"]
async fn test_a_revision_supersedes_the_proposal_and_its_sent_quotations() {
    let pool = pool().await;
    let (doc_id, me, qids, _) = make_doc(&pool, true).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1")
        .bind(qids[0])
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let mut doc = get_doc(&mut tx, doc_id).await;
    let new = core_::revise(&mut tx, &mut doc, &me, "Scope revised to 15 laptops")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut db = pool.acquire().await.unwrap();
    let old = get_doc(&mut db, doc_id).await;
    assert!(
        old.status == "superseded"
            && new.revision == 2
            && new.revision_group.as_deref() == Some(old.number.as_str())
    );
    assert!(
        new.full_number.as_deref().unwrap().ends_with("Rev. 2")
            && new.previous_version_id == Some(old.id)
    );
    let qstatus: String = sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1")
        .bind(qids[0])
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(qstatus, "superseded");
    let linked = core_::quotations_of(&mut db, new.id).await.unwrap();
    assert!(linked.len() == 1 && linked[0].0.id != qids[0]);
    assert_eq!(
        core_::blocks_of(&mut db, new.id).await.unwrap().len(),
        core_::blocks_of(&mut db, doc_id).await.unwrap().len()
    );
    let mut old = old;
    assert!(core_::revise(&mut db, &mut old, &me, "again")
        .await
        .is_err());
}

#[tokio::test]
async fn test_the_pdf_renders_markdown_images_coverage_and_the_quotation_totals() {
    let pool = pool().await;
    let (doc_id, _, qids, _) = make_doc(&pool, true).await;
    Mock::given(method("GET"))
        .and(path("/granary-documents/documents/x/y.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
        .mount(minio().await)
        .await;
    let mut tx = pool.begin().await.unwrap();
    let doc = get_doc(&mut tx, doc_id).await;
    let asset_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sales_document_assets (id, document_id, kind, mime, status, storage_key, caption, checks, attempts, promoted) \
         VALUES ($1, $2, 'upload', 'image/png', 'ready', 'documents/x/y.png', 'Site', '[]', 0, false)",
    )
    .bind(asset_id)
    .bind(doc_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let blocks = vec![
        json!({"section": "executive_summary", "block_type": "text", "data": {"md": "**Laptop** rollout for *15* staff.\n\n<script>x</script>"}}),
        json!({"section": "requirements", "block_type": "coverage", "data": {"rows": [
            {"requirement": "Single sign-on", "status": "included", "notes": "standard"},
            {"requirement": "Backups", "status": "optional_custom", "notes": "add-on"}]}}),
        json!({"section": "solution", "block_type": "image", "data": {"asset_id": asset_id.to_string(), "caption": "The site"}}),
        json!({"section": "commercial", "block_type": "commercial", "data": {"total": "999"}}),
        json!({"section": "approval", "block_type": "stamp", "data": {}}),
    ];
    core_::replace_blocks(&mut tx, &doc, &blocks).await.unwrap();
    tx.commit().await.unwrap();
    let mut db = pool.acquire().await.unwrap();
    let company = granary_services::verification::render::company_block(&mut db)
        .await
        .unwrap();
    let (template, data, facts) = render::build(&mut db, &doc, &company, None).await.unwrap();
    let total: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT total FROM quotations WHERE id = $1")
            .bind(qids[0])
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(template == "proposal" && data["pdf_header"] == "Confidential");
    let section = |title: &str| {
        data["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["title"] == title)
            .unwrap()
            .clone()
    };
    let text = section("Executive Summary")["blocks"][0]["html"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("<strong>Laptop</strong>") && !text.contains("<script>"));
    let cov = section("Requirement Coverage")["blocks"][0]["coverage"]["rows"].clone();
    let labels: Vec<&str> = cov
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status_label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Included", "Optional"]);
    let img = section("Proposed Solution")["blocks"][0]["image"].clone();
    assert!(
        img["src"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
            && img["caption"] == "Figure 1. The site"
    );
    let com = section("Commercial Summary")["blocks"][0]["commercial"].clone();
    let total_row = com["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r.get("total").is_some())
        .unwrap()
        .clone();
    let cell = total_row["cells"][1]["text"].as_str().unwrap();
    assert!(cell.contains("30.000.000") || cell.contains("30,000,000")); // = the quotation, not the typed 999
    assert!(total.to_string().starts_with("30000000"));
    assert!(facts
        .iter()
        .any(|(k, v)| k == "Customer" && v.as_deref() == Some("Pytest Corp")));
}

#[tokio::test]
async fn test_costs_never_reach_a_customer_context_or_the_pdf() {
    // (the services/nary/context.FORBIDDEN_KEYS half of this test is wave 3)
    let pool = pool().await;
    let (doc_id, _, _, _) = make_doc(&pool, false).await;
    let mut db = pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO nary_cost_sheets (id, document_id, content, currency, status) VALUES ($1, $2, $3, 'JPY', 'draft')")
        .bind(Uuid::new_v4())
        .bind(doc_id)
        .bind(json!({"summary": {"likely": {"cost": "123456789", "price_at_floor": "987654321"}}}))
        .execute(&mut *db)
        .await
        .unwrap();
    let doc = get_doc(&mut db, doc_id).await;
    let company = granary_services::verification::render::company_block(&mut db)
        .await
        .unwrap();
    let (_, data, facts) = render::build(&mut db, &doc, &company, None).await.unwrap();
    let blob = data.to_string() + &format!("{facts:?}");
    assert!(!blob.contains("123456789") && !blob.contains("987654321"));
}

// ─── White-box: errors, links, fingerprint, the image store ───────────────────

#[tokio::test]
async fn replace_blocks_refuses_what_python_refuses() {
    let pool = pool().await;
    let (doc_id, _, _, _) = make_doc(&pool, false).await;
    let mut db = pool.acquire().await.unwrap();
    let mut doc = get_doc(&mut db, doc_id).await;
    let e = core_::replace_blocks(
        &mut db,
        &doc,
        &[json!({"section": "nope", "block_type": "text"})],
    )
    .await
    .unwrap_err();
    assert_eq!(detail(&e), (422, "Unknown section 'nope'".into()));
    let e = core_::replace_blocks(&mut db, &doc, &[json!({"block_type": "text"})])
        .await
        .unwrap_err();
    assert_eq!(detail(&e), (422, "Unknown section 'None'".into()));
    let e = core_::replace_blocks(
        &mut db,
        &doc,
        &[json!({"section": "executive_summary", "type": "video"})],
    )
    .await
    .unwrap_err();
    assert_eq!(detail(&e), (422, "Unknown block type 'video'".into()));
    let e = core_::replace_blocks(
        &mut db,
        &doc,
        &[json!({"section": "requirements", "block_type": "coverage", "data": {"rows": [{"status": "maybe"}]}})],
    )
    .await
    .unwrap_err();
    assert_eq!(
        detail(&e),
        (
            422,
            "Coverage status must be one of included, optional_custom, not_in_scope, to_confirm"
                .into()
        )
    );
    // "type" works as "block_type"; commercial/stamp data is never stored
    core_::replace_blocks(
        &mut db,
        &doc,
        &[
            json!({"section": "executive_summary", "type": "text", "data": {"md": "x"}}),
            json!({"section": "commercial", "block_type": "commercial", "data": {"total": "999"}}),
        ],
    )
    .await
    .unwrap();
    let blocks = core_::blocks_of(&mut db, doc_id).await.unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].data, json!({"md": "x"}));
    assert_eq!(blocks[1].data, json!({}));
    doc.status = "approved".into();
    let e = core_::replace_blocks(&mut db, &doc, &[]).await.unwrap_err();
    assert_eq!(
        detail(&e),
        (409, "A approved proposal can't be edited: revise it".into())
    );
}

#[tokio::test]
async fn revise_checks_the_reason_and_the_status_first() {
    let pool = pool().await;
    let (doc_id, me, _, _) = make_doc(&pool, false).await;
    let mut db = pool.acquire().await.unwrap();
    let mut doc = get_doc(&mut db, doc_id).await;
    let e = core_::revise(&mut db, &mut doc, &me, "  \u{3000}")
        .await
        .unwrap_err();
    assert_eq!(
        detail(&e),
        (
            400,
            "Say why it is revised (it is printed on the cover)".into()
        )
    );
    doc.status = "superseded".into();
    let e = core_::revise(&mut db, &mut doc, &me, "why")
        .await
        .unwrap_err();
    assert_eq!(
        detail(&e),
        (
            409,
            "This revision is already superseded; revise the latest one".into()
        )
    );
}

#[tokio::test]
async fn link_quotation_checks_the_deal_and_links_once() {
    let pool = pool().await;
    let (doc_id, _, qids, deal) = make_doc(&pool, true).await;
    let mut db = pool.acquire().await.unwrap();
    let doc = get_doc(&mut db, doc_id).await;
    let e = core_::link_quotation(&mut db, &doc, &Uuid::new_v4().to_string(), None)
        .await
        .unwrap_err();
    assert_eq!(detail(&e), (404, "Quotation not found".into()));
    let other_cust = customer(&mut db).await;
    let other_deal = deal_for(&mut db, &other_cust).await;
    let foreign = quotation_for(&mut db, &other_deal, &other_cust, "draft").await;
    let e = core_::link_quotation(&mut db, &doc, &foreign.to_string(), None)
        .await
        .unwrap_err();
    assert_eq!(
        detail(&e),
        (400, "That quotation belongs to another deal".into())
    );
    core_::link_quotation(&mut db, &doc, &qids[0].to_string(), Some("Option A"))
        .await
        .unwrap();
    let cust = customer(&mut db).await;
    let second = quotation_for(&mut db, &deal, &cust, "draft").await;
    core_::link_quotation(
        &mut db,
        &doc,
        &second.simple().to_string(),
        Some("Option B"),
    )
    .await
    .unwrap();
    let linked = core_::quotations_of(&mut db, doc_id).await.unwrap();
    assert_eq!(linked.len(), 2);
    assert_eq!(linked[0].0.id, qids[0]);
    assert_eq!(linked[0].1, None); // the existing link is kept as it was
    assert_eq!(linked[1].1.as_deref(), Some("Option B"));
}

#[tokio::test]
async fn the_fingerprint_reads_the_totals_with_their_scale_and_changes_with_any_edit() {
    let pool = pool().await;
    let (doc_id, _, _, _) = make_doc(&pool, true).await;
    let mut db = pool.acquire().await.unwrap();
    let doc = get_doc(&mut db, doc_id).await;
    let quotes = core_::quotations_of(&mut db, doc_id).await.unwrap();
    assert_eq!(core_::str_dec(quotes[0].0.total), "30000000.00"); // str(Decimal) of NUMERIC(18,2)
    let before = core_::fingerprint(&mut db, &doc).await.unwrap();
    assert_eq!(before.len(), 64);
    assert_eq!(before, core_::fingerprint(&mut db, &doc).await.unwrap());
    let mut blocks: Vec<Value> = core_::blocks_of(&mut db, doc_id)
        .await
        .unwrap()
        .iter()
        .map(|b| json!({"section": b.section, "block_type": b.block_type, "data": b.data}))
        .collect();
    blocks[0]["data"] = json!({"md": "changed"});
    core_::replace_blocks(&mut db, &doc, &blocks).await.unwrap();
    assert_ne!(before, core_::fingerprint(&mut db, &doc).await.unwrap());
}

#[tokio::test]
async fn the_image_store_puts_into_the_documents_bucket_and_reads_back() {
    let server = minio().await;
    let _ = pool().await;
    Mock::given(method("HEAD"))
        .and(path("/granary-documents"))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/granary-documents"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/granary-documents/documents/doc-1/[0-9a-f]{32}\.jpg$",
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(server)
        .await;
    let key = assets::put("doc-1", b"jpegdata", "image/jpeg")
        .await
        .unwrap();
    assert!(key.starts_with("documents/doc-1/") && key.ends_with(".jpg"));
    let reqs = server.received_requests().await.unwrap();
    let put = reqs
        .iter()
        .rev()
        .find(|r| r.method.as_str() == "PUT" && r.url.path().ends_with(".jpg"))
        .unwrap();
    assert_eq!(put.body, b"jpegdata");
    assert!(put
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("AWS4-HMAC-SHA256 Credential="));
    assert_eq!(put.headers.get("content-type").unwrap(), "image/jpeg");
    let e = assets::put("doc-1", b"x", "image/gif").await.unwrap_err();
    assert_eq!(detail(&e), (415, "Images must be PNG, JPEG or WebP".into()));
    let e = assets::put("doc-1", &vec![0u8; 8 * 1024 * 1024 + 1], "image/png")
        .await
        .unwrap_err();
    assert_eq!(detail(&e), (413, "An image can be 8 MB at most".into()));
    assert_eq!(
        assets::data_uri(b"ab", "image/webp"),
        "data:image/webp;base64,YWI="
    );
}
