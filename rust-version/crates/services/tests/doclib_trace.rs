//! doclib A2: the fingerprint log and the trace of a leaked copy, customer and partner copies (deliver.rs),
//! and the internal price list (price_list.rs) against a fake PDF service.

mod support;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_services::doc_library::deliver::{self, Recipient, Sender};
use granary_services::doc_library::fingerprint::{self, Kind, MatchedBy, Who};
use granary_services::doc_library::{self as lib, price_list, Filters, Scope};
use granary_services::pdftools::{Document, Point, SaveOptions, TextOptions};
use granary_services::storage::{self, ObjectStore, StoreError};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

static DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static STORE: OnceLock<Arc<MemStore>> = OnceLock::new();
static PDF_BODIES: Mutex<Vec<Value>> = Mutex::new(Vec::new());
/// Tests that generate the price list share one document: one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn two_page_pdf(label: &str) -> Vec<u8> {
    let mut d = Document::new().unwrap();
    for i in 0..2 {
        let p = d.new_page(595.0, 842.0).unwrap();
        d.insert_text(
            p,
            Point::new(72.0, 100.0),
            &format!("{label} page {}", i + 1),
            &TextOptions::default(),
        )
        .unwrap();
    }
    d.tobytes(SaveOptions {
        garbage: 1,
        deflate: true,
        clean: false,
    })
    .unwrap()
}

/// The PDF service stand-in: records each /render body, answers with a one-page PDF. A thread that outlives
/// the per-test runtimes.
fn pdf_service() -> String {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let pdf = two_page_pdf("Price list");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let pdf = pdf.clone();
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let mut body = Vec::new();
                    loop {
                        let n = s.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            while buf.len() < pos + 4 + len {
                                let n = s.read(&mut chunk).unwrap_or(0);
                                if n == 0 {
                                    break;
                                }
                                buf.extend_from_slice(&chunk[..n]);
                            }
                            body = buf[pos + 4..].to_vec();
                            break;
                        }
                    }
                    if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                        PDF_BODIES.lock().unwrap().push(v);
                    }
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/pdf\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        pdf.len()
                    );
                    let _ = s.write_all(head.as_bytes());
                    let _ = s.write_all(&pdf);
                });
            }
        });
        url
    })
    .clone()
}

fn store() -> Arc<MemStore> {
    STORE
        .get_or_init(|| {
            let s = Arc::new(MemStore::default());
            storage::set_store(Some(s.clone()));
            s
        })
        .clone()
}

async fn pool() -> PgPool {
    std::env::set_var("PDF_SERVICE_URL", pdf_service());
    store();
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("doclib_trace").await;
            pool.close().await;
            std::env::var("DATABASE_URL").unwrap()
        })
        .await;
    PgPool::connect(url).await.expect("connect")
}

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

fn status(e: &ApiError) -> u16 {
    match e {
        ApiError::Http { status, .. } => status.as_u16(),
        other => panic!("not an HTTP error: {other:?}"),
    }
}

async fn actor(pool: &PgPool, id: Uuid) -> AuthUser {
    let mut c = pool.acquire().await.unwrap();
    granary_services::nary::roles::load_actor(&mut c, id)
        .await
        .unwrap()
        .expect("actor")
}

async fn superadmin(pool: &PgPool) -> AuthUser {
    let id: Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superadmin = true AND is_active = true ORDER BY created_at LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    actor(pool, id).await
}

async fn staff(pool: &PgPool, name: &str) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password, password_changed_at) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal', true, false, now())",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("doclib")))
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
    let mut user = actor(pool, id).await;
    user.perms = ["documents:view".to_string()].into_iter().collect();
    user
}

/// A document with one live version holding `file` (status may be changed by the caller).
async fn doc_with_live(
    pool: &PgPool,
    audiences: &[&str],
    kind: &str,
    file: &[u8],
    mime: &str,
) -> (lib::Document, lib::Version) {
    let did = Uuid::new_v4();
    let vid = Uuid::new_v4();
    let cat: Uuid = sqlx::query_scalar("SELECT id FROM doc_categories WHERE key = 'products'")
        .fetch_one(pool)
        .await
        .unwrap();
    let auds: Vec<String> = audiences.iter().map(|s| s.to_string()).collect();
    sqlx::query(
        "INSERT INTO library_documents (id, title, category_id, audiences, language, kind) \
         VALUES ($1, $2, $3, $4, 'en', $5)",
    )
    .bind(did)
    .bind(u("Brochure"))
    .bind(cat)
    .bind(&auds)
    .bind(kind)
    .execute(pool)
    .await
    .unwrap();
    let key = format!("doclib/test/{vid}");
    store()
        .blobs
        .lock()
        .unwrap()
        .insert(key.clone(), file.to_vec());
    sqlx::query(
        "INSERT INTO library_document_versions (id, document_id, label, change_note, status, file_object_key, \
         file_name, file_mime, file_size, file_sha256, went_live_at) \
         VALUES ($1, $2, 'v1', 'First version', 'live', $3, 'brochure.pdf', $4, $5, $6, now())",
    )
    .bind(vid)
    .bind(did)
    .bind(&key)
    .bind(mime)
    .bind(file.len() as i64)
    .bind(fingerprint::sha256_hex(file))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE library_documents SET current_version_id = $2 WHERE id = $1")
        .bind(did)
        .bind(vid)
        .execute(pool)
        .await
        .unwrap();
    let mut c = pool.acquire().await.unwrap();
    (
        lib::load_document(&mut c, did).await.unwrap().unwrap(),
        lib::load_version(&mut c, vid).await.unwrap().unwrap(),
    )
}

#[tokio::test]
async fn trace_by_file_by_id_and_by_a_resaved_copy() {
    let pool = pool().await;
    let ana = staff(&pool, "Ana Wilson").await;
    let (_doc, v) = doc_with_live(
        &pool,
        &["internal"],
        "uploaded",
        &two_page_pdf("Profile"),
        "application/pdf",
    )
    .await;
    let mut c = pool.acquire().await.unwrap();
    let who = Who {
        user_id: Some(ana.id),
        name: Some(ana.name.clone()),
        channel: Some("web".into()),
        ip: Some("10.0.0.7".into()),
        ..Default::default()
    };
    let bytes = lib::read_object(&v.file_object_key).await.unwrap();
    let (copy, issued) = fingerprint::stamped_copy(
        &mut c,
        Kind::StaffDownload,
        &v,
        &who,
        &bytes,
        "application/pdf",
    )
    .await
    .unwrap();
    assert!(
        issued.id.starts_with("ID-") && issued.id.len() == 13,
        "{}",
        issued.id
    );
    assert!(
        issued
            .footer_text
            .starts_with("Downloaded by Ana Wilson · ")
            && issued
                .footer_text
                .ends_with(&format!("· Confidential · {}", issued.id)),
        "{}",
        issued.footer_text
    );
    let text = Document::open(&copy).unwrap().get_text(1).unwrap();
    assert!(text.contains(&issued.footer_text), "{text}");

    // by the file itself
    let hit = fingerprint::trace_file(&mut c, &copy)
        .await
        .unwrap()
        .expect("hit");
    assert_eq!(hit.fingerprint, issued.id);
    assert_eq!(hit.matched_by, MatchedBy::Fingerprint);
    assert_eq!(hit.kind, "staff_download");
    assert_eq!(hit.user.as_ref().unwrap().name, "Ana Wilson");
    assert_eq!(hit.version.label, "v1");
    assert_eq!(hit.ip.as_deref(), Some("10.0.0.7"));
    assert_eq!(hit.channel.as_deref(), Some("web"));

    // by the id, typed loosely
    let typed = issued.id.to_lowercase().replace("id-", "id ");
    let hit = fingerprint::trace_id(&mut c, &typed)
        .await
        .unwrap()
        .expect("hit by id");
    assert_eq!(hit.fingerprint, issued.id);
    assert_eq!(hit.matched_by, MatchedBy::Id);
    assert!(fingerprint::trace_id(&mut c, "ID-ZZZZZZZZZZ")
        .await
        .unwrap()
        .is_none());

    // a copy re-saved without its metadata: the invisible line still names it
    let mut d = Document::open(&copy).unwrap();
    d.clear_info().unwrap();
    let resaved = d
        .tobytes(SaveOptions {
            garbage: 4,
            deflate: true,
            clean: true,
        })
        .unwrap();
    assert_ne!(
        fingerprint::sha256_hex(&resaved),
        fingerprint::sha256_hex(&copy)
    );
    let hit = fingerprint::trace_file(&mut c, &resaved)
        .await
        .unwrap()
        .expect("hit resaved");
    assert_eq!(hit.fingerprint, issued.id);

    // the log holds the hash of the stamped copy
    let sha: Option<String> =
        sqlx::query_scalar("SELECT file_sha256 FROM library_downloads WHERE fingerprint = $1")
            .bind(&issued.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        sha.as_deref(),
        Some(fingerprint::sha256_hex(&copy).as_str())
    );
}

#[tokio::test]
async fn trace_falls_back_to_the_file_hash() {
    let pool = pool().await;
    let (_doc, v) = doc_with_live(
        &pool,
        &["internal"],
        "uploaded",
        &two_page_pdf("Hash"),
        "application/pdf",
    )
    .await;
    // a copy whose marks can't be read (here: plain bytes) but that we logged
    let bytes = format!("unreadable copy {}", Uuid::new_v4()).into_bytes();
    let fp = fingerprint::new_id();
    sqlx::query(
        "INSERT INTO library_downloads (id, fingerprint, version_id, document_id, kind, recipient_company, channel, \
         file_sha256) VALUES ($1, $2, $3, $4, 'customer_send', 'Hash Ltd', 'email', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(&fp)
    .bind(v.id)
    .bind(v.document_id)
    .bind(fingerprint::sha256_hex(&bytes))
    .execute(&pool)
    .await
    .unwrap();
    let mut c = pool.acquire().await.unwrap();
    let hit = fingerprint::trace_file(&mut c, &bytes)
        .await
        .unwrap()
        .expect("hash hit");
    assert_eq!(hit.fingerprint, fp);
    assert_eq!(hit.matched_by, MatchedBy::FileHash);
    assert_eq!(hit.recipient_company.as_deref(), Some("Hash Ltd"));
    assert!(fingerprint::trace_file(&mut c, b"never seen")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn customer_copies_follow_the_hard_rules() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let mut c = pool.acquire().await.unwrap();
    let pdf = two_page_pdf("Datasheet");
    // a customer document: stamped for the company, logged as a Nary send
    let (_d, v) = doc_with_live(&pool, &["customer"], "uploaded", &pdf, "application/pdf").await;
    let blob = deliver::customer_copy(
        &mut c,
        &v,
        Some("Vandelay Industries Ltd"),
        "whatsapp",
        Sender::Nary { confirmed_by: None },
    )
    .await
    .unwrap();
    assert_eq!(blob.filename, "brochure.pdf");
    let text = Document::open(&blob.data).unwrap().get_text(0).unwrap();
    assert!(
        text.contains("Prepared for Vandelay Industries Ltd · "),
        "{text}"
    );
    let hit = fingerprint::trace_file(&mut c, &blob.data)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hit.kind, "nary_send");
    assert_eq!(
        hit.recipient_company.as_deref(),
        Some("Vandelay Industries Ltd")
    );
    assert_eq!(hit.channel.as_deref(), Some("whatsapp"));
    let admin = superadmin(&pool).await;
    let d2 = deliver::customer_copy_for(
        &mut c,
        &v,
        &Recipient {
            company: Some("Other Ltd".into()),
            name: Some("Bob".into()),
            partner_account_id: None,
        },
        "email",
        Sender::Staff(admin.id),
    )
    .await
    .unwrap();
    assert_ne!(d2.fingerprint, hit.fingerprint);
    let kind: String =
        sqlx::query_scalar("SELECT kind FROM library_downloads WHERE fingerprint = $1")
            .bind(&d2.fingerprint)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kind, "customer_send");

    // never an internal document
    let (_d, internal) =
        doc_with_live(&pool, &["internal"], "uploaded", &pdf, "application/pdf").await;
    let e = deliver::customer_copy(
        &mut c,
        &internal,
        Some("X Ltd"),
        "email",
        Sender::Nary { confirmed_by: None },
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403);
    // never the price list
    let (_d, pl) = doc_with_live(&pool, &["internal"], "price_list", &pdf, "application/pdf").await;
    let e = deliver::customer_copy(
        &mut c,
        &pl,
        Some("X Ltd"),
        "email",
        Sender::Nary { confirmed_by: None },
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403);
    // (the price list test creates the real one)
    sqlx::query("UPDATE library_documents SET is_deleted = true WHERE id = $1")
        .bind(pl.document_id)
        .execute(&pool)
        .await
        .unwrap();
    // a partner-only document only to a partner account
    let (_d, partner_only) =
        doc_with_live(&pool, &["partner"], "uploaded", &pdf, "application/pdf").await;
    let e = deliver::customer_copy(
        &mut c,
        &partner_only,
        Some("X Ltd"),
        "email",
        Sender::Nary { confirmed_by: None },
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403);
    // only the live version
    sqlx::query("UPDATE library_document_versions SET status = 'superseded' WHERE id = $1")
        .bind(v.id)
        .execute(&pool)
        .await
        .unwrap();
    let old = lib::load_version(&mut c, v.id).await.unwrap().unwrap();
    let e = deliver::customer_copy(
        &mut c,
        &old,
        Some("X Ltd"),
        "email",
        Sender::Nary { confirmed_by: None },
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM library_downloads WHERE version_id = ANY($1)")
            .bind(vec![internal.id, pl.id, partner_only.id])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0, "refused copies leave no log rows");
}

#[tokio::test]
async fn partner_portal_sees_partner_documents_stamped_with_the_partner_company() {
    let pool = pool().await;
    let mut c = pool.acquire().await.unwrap();
    let pdf = two_page_pdf("Partner kit");
    let (pdoc, pv) = doc_with_live(&pool, &["partner"], "uploaded", &pdf, "application/pdf").await;
    let (cdoc, _) = doc_with_live(&pool, &["customer"], "uploaded", &pdf, "application/pdf").await;
    let (idoc, _) = doc_with_live(&pool, &["internal"], "uploaded", &pdf, "application/pdf").await;
    let res = lib::search(
        &mut c,
        &Scope::Audiences(vec!["partner".into()]),
        None,
        &Filters {
            only_live: true,
            limit: Some(200),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let ids: Vec<Uuid> = res.items.iter().map(|d| d.id).collect();
    assert!(ids.contains(&pdoc.id));
    assert!(!ids.contains(&cdoc.id) && !ids.contains(&idoc.id));

    let partner_id = Uuid::new_v4();
    let portal_user = superadmin(&pool).await;
    let out = deliver::partner_copy(
        &mut c,
        &pv,
        partner_id,
        "Partner Solutions Ltd",
        portal_user.id,
        Some("1.2.3.4".into()),
        None,
    )
    .await
    .unwrap();
    let text = Document::open(&out.file.data).unwrap().get_text(1).unwrap();
    assert!(
        text.contains("Prepared for Partner Solutions Ltd · "),
        "{text}"
    );
    let row: (String, Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT kind, partner_account_id, channel FROM library_downloads WHERE fingerprint = $1",
    )
    .bind(&out.fingerprint)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "partner_download".into(),
            Some(partner_id),
            Some("portal".into())
        )
    );
    // a customer-only document is not a partner download
    let cv = lib::current_live(&mut c, cdoc.id).await.unwrap().unwrap();
    let e = deliver::partner_copy(
        &mut c,
        &cv,
        partner_id,
        "Partner Ltd",
        portal_user.id,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 404);
}

#[tokio::test]
async fn price_list_becomes_a_new_live_internal_version() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let mut c = pool.acquire().await.unwrap();
    let pid = Uuid::new_v4();
    let sku = u("LAPTOP-S25");
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, family, product_type, sale_model, price_metric, pricing_mode, \
         is_whitelabel, is_active, is_orderable, is_deleted, unit) \
         VALUES ($1, $2, 'Laptop Server 25', 'laptop', 'hardware', 'one_time', 'unit', 'list', false, true, true, false, 'unit')",
    )
    .bind(pid)
    .bind(&sku)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) \
         VALUES (gen_random_uuid(), $1, 'JPY', 25000000, CURRENT_DATE - 1)",
    )
    .bind(pid)
    .execute(&pool)
    .await
    .unwrap();
    let owner = superadmin(&pool).await;
    PDF_BODIES.lock().unwrap().clear();
    let (doc, v1) = price_list::generate(&mut c, &owner).await.unwrap();
    assert_eq!(doc.kind, "price_list");
    assert_eq!(doc.audiences, vec!["internal".to_string()]);
    assert_eq!(doc.title, "Price List");
    let cat: String = sqlx::query_scalar("SELECT key FROM doc_categories WHERE id = $1")
        .bind(doc.category_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cat, "pricing");
    assert_eq!(v1.status, "live");
    assert_eq!(doc.current_version_id, Some(v1.id));
    assert!(v1.change_note.starts_with("Generated from the catalog on "));
    // what the PDF service got: the template and the catalog's list price, grouped by family
    let body = PDF_BODIES
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("render call");
    assert_eq!(body["template"], "price_list");
    let data = &body["data"];
    assert_eq!(data["internal_mark"], "Internal");
    let jpy = data["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["currency"] == "JPY")
        .expect("JPY section");
    let fam = jpy["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["family"] == "Laptop")
        .expect("family");
    let item = fam["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["sku"] == sku.as_str())
        .expect("item");
    assert_eq!(item["price"], "JPY 25,000,000");
    assert_eq!(item["terms"], "per unit");

    // again: a second live version, the first superseded, the same document
    let (doc2, v2) = price_list::generate(&mut c, &owner).await.unwrap();
    assert_eq!(doc2.id, doc.id);
    assert_eq!(v2.status, "live");
    assert_ne!(v2.label, v1.label);
    let old = lib::load_version(&mut c, v1.id).await.unwrap().unwrap();
    assert_eq!(old.status, "superseded");
    // never sent outside
    let e = deliver::customer_copy(
        &mut c,
        &v2,
        Some("X Ltd"),
        "email",
        Sender::Nary { confirmed_by: None },
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403);
    // staff without documents:create may not generate it
    let reader = staff(&pool, "Reader").await;
    let e = price_list::generate(&mut c, &reader).await.unwrap_err();
    assert_eq!(status(&e), 403);
}
