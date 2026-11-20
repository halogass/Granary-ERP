//! 23 Oct 2026, Nary's R&D backlog from the Owner's chats (the Hooli Consulting note, the Globex Group proposal
//! and email), the internal paths (the ERP chat and Nary on WhatsApp). your AI client (MCP) at full power and the
//! REST routes are crates/api/tests/nary_backlog_oct23_routes.rs.
//!
//! - get_activity: a 20,000-character Markdown note reads back identical, in parts (offset / next_offset),
//!   with its author, links and the internal flag; get_activity_history marks it truncated with read_with;
//!   another salesperson's record is refused; a CRM AI card over crm.activity.max_chars is refused (the
//!   reason, not "Tool failed").
//! - One file per document: a proposal with one and then two linked quotations plus the quotation on its own:
//!   the card lists the bundled quotations and warns, nary_documents gives each PDF once, and nary_mark_sent
//!   records one send each.
//! - render_document_page: a draft proposal page and a quotation page come back as a JPEG with the page count;
//!   the verified copy when one was issued; a page past the end is a clear error.

mod nary_n6_common;
mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog::run_tool;
use granary_services::storage::{ObjectStore, StoreError};
use granary_services::verification::delivery;
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

// ─── fixtures ────────────────────────────────────────────────────────────────

/// A real PDF of `pages` blank A4 pages (picture_of draws them).
fn real_pdf(pages: usize) -> Vec<u8> {
    let mut d = granary_services::pdftools::Document::new().unwrap();
    for _ in 0..pages {
        d.new_page(595.0, 842.0).unwrap();
    }
    d.tobytes(Default::default()).unwrap()
}

static RENDERS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

struct PdfService;

impl Respond for PdfService {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if req.url.path() == "/render" {
            RENDERS
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&req.body).unwrap_or(Value::Null));
            return ResponseTemplate::new(200).set_body_bytes(real_pdf(3));
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

/// The tests share process-wide state (the Nary session pool, the PDF fake, the delivery seams): one at a time.
/// (23 Oct: run in parallel, the dedupe test waited for ever on another test's database.)
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    pdf_service();
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
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

/// A person with exactly these permissions (not a superadmin).
async fn person(db: &mut PgConnection, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password, password_changed_at) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal', true, false, now())",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("oct23")))
    .bind(u("Sales"))
    .execute(&mut *db)
    .await
    .unwrap();
    let mut user = auth_user(db, id).await;
    user.perms = perms.iter().map(|p| p.to_string()).collect();
    user
}

/// The Hooli Consulting kind of note: long Markdown with headings, lists, a table and non-ASCII text,
/// exactly `n` characters, ending with the decision that was "lost".
fn long_note(n: usize) -> String {
    let mut s = String::from(
        "## Decisions (Hooli Consulting)\n\n| Item | Owner |\n|---|---|\n| Scope | Owner |\n\n",
    );
    let tail = "\n5. Reply to Franklin.";
    let mut i = 0;
    loop {
        i += 1;
        let line = format!(
            "{i}. Point {i}: café meeting, the customer asked about Tokyo and Osaka sites.\n"
        );
        if s.chars().count() + line.chars().count() + tail.chars().count() > n {
            break;
        }
        s.push_str(&line);
    }
    while s.chars().count() + tail.chars().count() < n {
        s.push('x');
    }
    s.push_str(tail);
    assert_eq!(s.chars().count(), n);
    s
}

async fn note_on(db: &mut PgConnection, deal: Uuid, author: Uuid, content: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, deal_id, activity_type, direction, subject, content, \
         scheduled_at, is_completed, created_by_id, created_at, updated_at) \
         VALUES ($1, 'deal', $2, $2, 'note', 'outbound', 'Call notes', $3, now(), false, $4, now(), now())",
    )
    .bind(id)
    .bind(deal)
    .bind(content)
    .bind(author)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn proposal(db: &mut PgConnection, deal: Uuid, status: &str) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let number = u("SP");
    sqlx::query(
        "INSERT INTO sales_documents (id, doc_type, number, revision, title, language, status, validity_days, \
         cover, deal_id, is_deleted) VALUES ($1, 'solution_proposal', $2, 1, 'Laptop rollout', 'en', $3, 30, '{}', $4, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(status)
    .bind(deal)
    .execute(&mut *db)
    .await
    .unwrap();
    (id, number)
}

async fn link(db: &mut PgConnection, doc: Uuid, q: Uuid, pos: i32) {
    sqlx::query(
        "INSERT INTO sales_document_quotations (id, document_id, quotation_id, position) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(doc)
    .bind(q)
    .bind(pos)
    .execute(&mut *db)
    .await
    .unwrap();
}

async fn number_of(db: &mut PgConnection, q: Uuid) -> String {
    sqlx::query_scalar("SELECT number FROM quotations WHERE id = $1")
        .bind(q)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

// ─── get_activity ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_long_note_reads_back_in_full_in_parts() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("oct23_activity").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let cust = customer(db, None, false, "en").await;
    let deal = deal_for(db, &cust, "qualified").await;
    let note = long_note(20_000);
    let id = note_on(db, deal, me_id, &note).await;

    // the history shows the preview and says it is longer
    let h = run_tool(
        db,
        &me,
        "get_activity_history",
        json!({"object_type": "deal", "record_id": deal.to_string()}),
    )
    .await;
    let item = h["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(id.to_string()))
        .cloned()
        .unwrap_or_else(|| panic!("{h}"));
    assert_eq!(item["truncated"], json!(true), "{item}");
    assert_eq!(item["length"], json!(20_000));
    assert_eq!(item["read_with"], json!("get_activity"));
    assert!(!item["preview"]
        .as_str()
        .unwrap()
        .contains("Reply to Franklin"));
    // a short note is not marked
    let short = note_on(db, deal, me_id, "All good.").await;
    let h = run_tool(
        db,
        &me,
        "get_activity_history",
        json!({"object_type": "deal", "record_id": deal.to_string()}),
    )
    .await;
    let s = h["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(short.to_string()))
        .cloned()
        .unwrap();
    assert!(s.get("truncated").is_none(), "{s}");

    // get_activity pages through it and the parts add up to the stored text
    let mut text = String::new();
    let mut offset = 0;
    let mut parts = 0;
    loop {
        let out = run_tool(
            db,
            &me,
            "get_activity",
            json!({"activity_id": id.to_string(), "offset": offset, "max_chars": 6000}),
        )
        .await;
        assert!(out.get("error").is_none(), "{out}");
        assert_eq!(out["length"], json!(20_000));
        assert_eq!(out["activity_type"], json!("note"));
        assert_eq!(out["subject"], json!("Call notes"));
        assert_eq!(out["internal"], json!(true), "a note is internal");
        assert!(out["internal_note"]
            .as_str()
            .unwrap()
            .contains("never quote"));
        assert_eq!(out["deal"]["id"], json!(deal.to_string()));
        assert!(out["deal"]["name"]
            .as_str()
            .unwrap()
            .starts_with("Pytest deal"));
        assert!(out["author"].is_string(), "{out}");
        text.push_str(out["content"].as_str().unwrap());
        parts += 1;
        match out.get("next_offset").and_then(Value::as_u64) {
            Some(n) => offset = n,
            None => break,
        }
    }
    assert_eq!(parts, 4);
    assert_eq!(text, note, "identical to what was saved");
    assert!(text.ends_with("5. Reply to Franklin."));

    // the default part is nary.files.read_max_chars (12000)
    let out = run_tool(
        db,
        &me,
        "get_activity",
        json!({"activity_id": id.to_string()}),
    )
    .await;
    assert_eq!(out["content"].as_str().unwrap().chars().count(), 12_000);
    assert_eq!(out["next_offset"], json!(12_000));

    // an unknown id, and another salesperson's record
    let out = run_tool(
        db,
        &me,
        "get_activity",
        json!({"activity_id": Uuid::new_v4().to_string()}),
    )
    .await;
    assert!(
        out["error"].as_str().unwrap().contains("No activity"),
        "{out}"
    );
    let other = person(db, &["crm:view_own"]).await;
    let owner = person(db, &["crm:view_own"]).await;
    sqlx::query("UPDATE deals SET assigned_to_id = $1 WHERE id = $2")
        .bind(owner.id)
        .bind(deal)
        .execute(&mut *db)
        .await
        .unwrap();
    let out = run_tool(
        db,
        &other,
        "get_activity",
        json!({"activity_id": id.to_string()}),
    )
    .await;
    assert!(
        out["error"].as_str().unwrap().contains("someone else"),
        "{out}"
    );
    let out = run_tool(
        db,
        &owner,
        "get_activity",
        json!({"activity_id": id.to_string()}),
    )
    .await;
    assert_eq!(out["length"], json!(20_000), "{out}");
}

#[tokio::test]
async fn a_note_over_the_limit_is_refused_never_cut() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("oct23_limit").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let cust = customer(db, None, false, "en").await;
    let deal = deal_for(db, &cust, "qualified").await;
    let old = set(db, &[("crm.activity.max_chars", "1000")]).await;
    let fields = |n: usize| {
        json!({"entity_type": "deal", "entity_id": deal.to_string(), "deal_id": deal.to_string(),
               "activity_type": "note", "subject": "Notes", "content": long_note(n)})
    };
    // the chat's CRM card: the reason, not "Tool failed"
    let out = run_tool(
        db,
        &me,
        "propose_record_change",
        json!({"object_type": "activity", "action": "create", "fields": fields(1001)}),
    )
    .await;
    let e = out["error"].as_str().unwrap_or_else(|| panic!("{out}"));
    assert!(
        e.contains("1001 characters") && e.contains("crm.activity.max_chars"),
        "{e}"
    );
    // the apply path (the AI client's create_crm_record at act and full power) is a 422
    let r = granary_services::crm::writes::apply_change(
        db,
        &me,
        "activity",
        "create",
        &fields(1001),
        None,
        "mcp",
    )
    .await;
    match r {
        Err(granary_services::crm::writes::WriteError::Api(e)) => {
            assert_eq!(e.status().as_u16(), 422, "{e:?}")
        }
        other => panic!("{other:?}"),
    }
    // at the limit it is stored in full
    let out = granary_services::crm::writes::apply_change(
        db,
        &me,
        "activity",
        "create",
        &fields(1000),
        None,
        "mcp",
    )
    .await
    .unwrap();
    let id = Uuid::parse_str(out["id"].as_str().unwrap_or_else(|| panic!("{out}"))).unwrap();
    let len: i32 = sqlx::query_scalar("SELECT length(content) FROM activities WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(len, 1000);
    restore(db, old).await;
}

// ─── one file per document ───────────────────────────────────────────────────

/// Verified PDFs without the PDF service.
fn fake_pdfs() {
    delivery::seams::reset();
    delivery::seams::set_verified_pdf(Some(Arc::new(|_db, t, e, _u| {
        Box::pin(async move { Ok((None, format!("%PDF-1.4 {t} {}", e.id()).into_bytes())) })
    })));
}

#[tokio::test]
async fn a_quotation_bundled_with_its_proposal_goes_once() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("oct23_dedupe").await;
    let db: &mut PgConnection = &mut conn;
    fake_pdfs();
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let cust = customer(db, Some("procurement@globex.test"), true, "en").await;
    let chat = cust.chat_id.clone().unwrap();
    let deal = deal_for(db, &cust, "proposal").await;
    let (doc, doc_number) = proposal(db, deal, "draft").await;
    // 1 Nov 2026: the proposal has no figures; the required-figures gate is not the subject here
    set(db, &[("proposal.visuals.default_figures", "[]")]).await;
    let qa = quotation_for(db, deal, &cust, None, "draft", 2, 15_000_000).await;
    link(db, doc, qa, 0).await;
    let qa_number = number_of(db, qa).await;

    // one linked quotation, given again on its own (the Globex email)
    let out = run_tool(
        db,
        &me,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": chat, "text": "Dear Sir, please find our proposal.",
               "attach": [{"type": "sales_document", "id": doc.to_string()},
                          {"type": "quotation", "id": qa_number}]}),
    )
    .await;
    let p = out["proposal"].clone();
    assert!(p.is_object(), "{out}");
    assert_eq!(
        p["attach"],
        json!([{"type": "sales_document", "id": doc.to_string()}]),
        "the quotation on its own is dropped: {p}"
    );
    let labels: Vec<String> = serde_json::from_value(p["attachments"].clone()).unwrap();
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert!(
        labels[0].contains(&doc_number.replace('/', "-"))
            || labels[0].starts_with("Solution Proposal"),
        "{labels:?}"
    );
    assert!(
        labels[1].starts_with("Quotation ") && labels[1].ends_with("(PDF, from the proposal)"),
        "{labels:?}"
    );
    let warning = p["warning"].as_str().unwrap_or_else(|| panic!("{p}"));
    assert!(
        warning.contains(&qa_number)
            && warning.contains("already included")
            && warning.contains("goes once"),
        "{warning}"
    );
    // the WhatsApp card text lists them too
    let shown = granary_services::nary::staff_chat::describe(&p);
    assert!(shown.contains("from the proposal"), "{shown}");

    // a second linked quotation: both listed
    let qb = quotation_for(db, deal, &cust, None, "draft", 1, 9_000_000).await;
    link(db, doc, qb, 1).await;
    let out = run_tool(
        db,
        &me,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": chat, "text": "Dear Sir, please find our proposal.",
               "attach": [{"type": "quotation", "id": qb.to_string()},
                          {"type": "sales_document", "id": doc.to_string()}]}),
    )
    .await;
    let p = out["proposal"].clone();
    let labels: Vec<String> = serde_json::from_value(p["attachments"].clone()).unwrap();
    assert_eq!(labels.len(), 3, "{labels:?}");
    assert_eq!(
        labels
            .iter()
            .filter(|l| l.ends_with("(PDF, from the proposal)"))
            .count(),
        2
    );
    assert_eq!(p["attach"].as_array().unwrap().len(), 1, "{p}");

    // the send path, also for a card made before this fix (the quotation on its own AND with the proposal)
    let items = vec![
        json!({"type": "quotation", "id": qa.to_string()}),
        json!({"type": "sales_document", "id": doc.to_string()}),
        json!({"type": "sales_document", "id": doc.to_string()}),
    ];
    let mut docs = delivery::nary_documents(db, &items, &me).await.unwrap();
    let mut ids: Vec<Uuid> = docs.iter().map(|d| d.entity.id()).collect();
    assert_eq!(
        ids.len(),
        3,
        "the proposal and its two quotations, once each"
    );
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
    delivery::nary_mark_sent(db, &mut docs, &me, "whatsapp", &chat, Some("wamid-1"))
        .await
        .unwrap();
    for q in [qa, qb] {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM domain_events WHERE type = 'quotation.sent' AND entity_id = $1",
        )
        .bind(q)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!(n, 1, "one send recorded for {q}");
    }
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM activities WHERE deal_id = $1 AND subject LIKE 'Solution Proposal %'",
    )
    .bind(deal)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(logged, 1, "the proposal's send is on the timeline once");
    delivery::seams::reset();
}

// ─── render_document_page ────────────────────────────────────────────────────

fn page_of(out: &Value) -> Vec<u8> {
    use base64::Engine as _;
    let imgs = out["_images"].as_array().unwrap_or_else(|| panic!("{out}"));
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0]["mime"], json!("image/jpeg"));
    base64::engine::general_purpose::STANDARD
        .decode(imgs[0]["data"].as_str().unwrap())
        .unwrap()
}

#[tokio::test]
async fn a_document_page_comes_back_as_a_picture() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("oct23_page").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let cust = customer(db, None, false, "en").await;
    let deal = deal_for(db, &cust, "proposal").await;
    let (doc, doc_number) = proposal(db, deal, "draft").await;
    let q = quotation_for(db, deal, &cust, None, "draft", 2, 15_000_000).await;
    let q_number = number_of(db, q).await;
    let store = Arc::new(MemStore::default());

    granary_services::storage::with_store(store.clone(), async {
        // a draft proposal, by number: nothing was issued, so the draft (and the answer says so)
        RENDERS.lock().unwrap().clear();
        let out = run_tool(
            db,
            &me,
            "render_document_page",
            json!({"type": "sales_document", "id": doc_number, "page": 2}),
        )
        .await;
        assert!(out.get("error").is_none(), "{out}");
        assert_eq!(out["which"], json!("draft"));
        assert!(out["note"].as_str().unwrap().contains("no verified copy"), "{out}");
        assert_eq!(out["pages"], json!(3));
        assert_eq!(out["page"], json!(2));
        let jpeg = page_of(&out);
        assert_eq!(&jpeg[..2], b"\xFF\xD8");
        let pic = image::load_from_memory(&jpeg).unwrap();
        assert!(pic.height() > pic.width(), "an A4 page, upright");
        {
            let r = RENDERS.lock().unwrap();
            assert_eq!(r.len(), 1);
            assert_eq!(r[0]["template"], json!("proposal"));
            assert_eq!(r[0]["footer"]["line"], json!("Draft, not for distribution"));
            assert_eq!(r[0]["footer"]["qr_data_uri"], json!(""));
        }
        let issued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM document_verifications WHERE entity_id = $1",
        )
        .bind(doc)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!(issued, 0, "a draft issues nothing");

        // a quotation draft, by id
        RENDERS.lock().unwrap().clear();
        let out = run_tool(
            db,
            &me,
            "render_document_page",
            json!({"type": "quotation", "id": q.to_string(), "which": "draft"}),
        )
        .await;
        assert_eq!(out["which"], json!("draft"), "{out}");
        assert_eq!(out["number"], json!(q_number));
        page_of(&out);
        assert_eq!(
            RENDERS.lock().unwrap()[0]["footer"]["line"],
            json!("Draft, not for distribution")
        );

        // the verified copy the customer got (one page), without drawing anything new
        let key = format!("verification/quotation/{}.pdf", Uuid::new_v4());
        store
            .blobs
            .lock()
            .unwrap()
            .insert(key.clone(), real_pdf(1));
        sqlx::query(
            "INSERT INTO document_verifications (id, entity_type, entity_id, revision, document_number, token, \
             short_code, status, issued_at, view_scope, allow_download, requires_login, facts, view_count, \
             pdf_object_key) VALUES ($1, 'quotation', $2, 1, $3, $4, 'QX7K2M', 'active', now(), 'metadata', true, \
             false, '{}', 0, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(q)
        .bind(&q_number)
        .bind(Uuid::new_v4())
        .bind(&key)
        .execute(&mut *db)
        .await
        .unwrap();
        RENDERS.lock().unwrap().clear();
        let out = run_tool(
            db,
            &me,
            "render_document_page",
            json!({"type": "quotation", "id": q_number}),
        )
        .await;
        assert_eq!(out["which"], json!("sent"), "{out}");
        assert_eq!(out["pages"], json!(1));
        assert_eq!(out["verified_copy"]["verification_code"], json!("QX7K2M"));
        assert!(RENDERS.lock().unwrap().is_empty(), "the stored copy, not a new render");
        page_of(&out);

        // a page past the end, an unknown number, a bad type
        let out = run_tool(
            db,
            &me,
            "render_document_page",
            json!({"type": "quotation", "id": q_number, "page": 5}),
        )
        .await;
        let e = out["error"].as_str().unwrap_or_else(|| panic!("{out}"));
        assert!(e.contains("has 1 page(s)"), "{e}");
        assert!(out.get("_images").is_none());
        let out = run_tool(
            db,
            &me,
            "render_document_page",
            json!({"type": "quotation", "id": "QUO/NOPE"}),
        )
        .await;
        assert!(out["error"].as_str().unwrap().contains("No quotation"), "{out}");
        let out = run_tool(
            db,
            &me,
            "render_document_page",
            json!({"type": "invoice", "id": q.to_string()}),
        )
        .await;
        assert!(out["error"].as_str().unwrap().contains("quotation or sales_document"));
    })
    .await;

    // read level: listed (a read tool), with its picture handled as an image tool
    use granary_services::ai::mcp_access as m;
    for scopes in [vec!["read"], vec!["full"], vec!["act", "owner"]] {
        let scopes: Vec<String> = scopes.into_iter().map(String::from).collect();
        let names: Vec<String> = m::allowed_tools(db, &me, &scopes)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        for n in [
            "render_document_page",
            "get_activity",
            "get_activity_history",
        ] {
            assert!(names.iter().any(|x| x == n), "{n} at {scopes:?}");
        }
    }
    assert!(granary_services::ai::tool_images::IMAGE_TOOLS.contains(&"render_document_page"));
    // a person without sales:view_quotations doesn't get it
    let crm_only = person(db, &["crm:view_own"]).await;
    let names: Vec<String> = m::allowed_tools(db, &crm_only, &["read".to_string()])
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(!names.iter().any(|x| x == "render_document_page"));
    assert!(names.iter().any(|x| x == "get_activity"));
}
