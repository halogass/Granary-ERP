//! 5 Nov 2026 (Stream A of the Owner-approved plan, migration 0162): videos in the document library, store once
//! from a CRM attachment, and the Owner's card that adds a product file (propose_product_file,
//! action catalog.product_file). your AI client (MCP) and the attachment delete are
//! crates/api/tests/library_videos_routes.rs.
//!
//! - A video version (MP4 judged by its bytes, its length kept, no text read); a customer copy is logged in
//!   library_downloads with the original bytes (its hash the original's) and the trace says videos are logged,
//!   not watermarked; a PDF is still stamped; a file named .mp4 that is not a video, a text file and a video
//!   over nary.upload.video_max_mb are refused.
//! - The card from a deal attachment: one stored object (the version references it, file_bucket assets), the
//!   product's library document opens it, a customer copy sends the same bytes, find_product_files lists the
//!   video with its length; a new version of that document with remove_from_record takes the attachment row
//!   off the deal and keeps its object.
//! - Anyone but the Owner is refused at the tool and at apply and does not see the tool; a wrong type is refused
//!   on propose; catalog.product_file is Ask, never promoted, confirmed twice.

mod nary_n6_common;
mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog::{run_tool, tools_for_user};
use granary_services::doc_library::{
    self as lib, deliver, fingerprint,
    versions::{self, NewDocument, NewFile},
};
use granary_services::nary::{
    actions, autonomy, catalog_changes as cc, product_files as pf, uploads,
};
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_n6_common::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// The tests share process-wide state (the store, the Nary session pool): one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Objects by (bucket, key), so a read from the wrong bucket fails.
#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl MemStore {
    fn count(&self) -> usize {
        self.blobs.lock().unwrap().len()
    }
    fn has(&self, key: &str) -> bool {
        self.blobs.lock().unwrap().keys().any(|(_, k)| k == key)
    }
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
        b: &str,
        key: &str,
        data: &[u8],
        _ct: &str,
    ) -> Result<(), StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .insert((b.to_string(), key.to_string()), data.to_vec());
        Ok(())
    }
    async fn get_object(&self, b: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .get(&(b.to_string(), key.to_string()))
            .cloned()
            .ok_or(StoreError {
                kind: "S3Error".into(),
                message: format!("{b}/{key}"),
            })
    }
    async fn remove_object(&self, b: &str, key: &str) -> Result<(), StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .remove(&(b.to_string(), key.to_string()));
        Ok(())
    }
}

// ─── synthetic media (as nary_video_files.rs) ────────────────────────────────

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// An MP4 (isom, H.264) of `seconds` and `total` bytes.
fn mp4(seconds: u32, total: usize) -> Vec<u8> {
    let mut ftyp = b"isom".to_vec();
    ftyp.extend_from_slice(&[0, 0, 2, 0]);
    for c in [b"isom", b"iso2", b"avc1", b"mp41"] {
        ftyp.extend_from_slice(c);
    }
    let mut out = boxed(b"ftyp", &ftyp);
    let mut mvhd = vec![0u8, 0, 0, 0];
    mvhd.extend_from_slice(&[0; 8]);
    mvhd.extend_from_slice(&1000u32.to_be_bytes());
    mvhd.extend_from_slice(&(seconds * 1000).to_be_bytes());
    mvhd.resize(mvhd.len() + 80, 0);
    let mut moov = boxed(b"mvhd", &mvhd);
    let stsd = boxed(
        b"stsd",
        &[&[0u8; 8][..], &boxed(b"avc1", &[0; 16])].concat(),
    );
    moov.extend(boxed(
        b"trak",
        &boxed(b"mdia", &boxed(b"minf", &boxed(b"stbl", &stsd))),
    ));
    out.extend(boxed(b"moov", &moov));
    let pad = total.saturating_sub(out.len() + 8);
    out.extend(boxed(b"mdat", &vec![0x11; pad]));
    out
}

fn pdf(text: &str) -> Vec<u8> {
    use granary_services::pdftools::{Document, Point, SaveOptions, TextOptions};
    let mut d = Document::new().unwrap();
    let p = d.new_page(595.0, 842.0).unwrap();
    d.insert_text(p, Point::new(50.0, 80.0), text, &TextOptions::default())
        .unwrap();
    d.tobytes(SaveOptions::default()).unwrap()
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

// ─── fixtures ────────────────────────────────────────────────────────────────

async fn setup(tag: &str) -> (PgPool, Arc<MemStore>) {
    let store = Arc::new(MemStore::default());
    storage::set_store(Some(store.clone()));
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    (pool, store)
}

async fn the_owner(db: &mut PgConnection) -> AuthUser {
    let id = owner(db).await;
    auth_user(db, id).await
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
    .bind(format!("{}@granary.test", u("libvid")))
    .bind(u("Executive"))
    .execute(&mut *db)
    .await
    .unwrap();
    let mut user = auth_user(db, id).await;
    user.perms = perms.iter().map(|p| p.to_string()).collect();
    user
}

async fn deal(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, value, currency, probability, is_deleted) \
         VALUES ($1, $2, 'discovery', 0, 'JPY', 10, false)",
    )
    .bind(id)
    .bind(u("Video deal"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A file on the deal's Attachments (as upload_file stores it): (attachment id, object key).
async fn attach(
    db: &mut PgConnection,
    me: &AuthUser,
    d: Uuid,
    name: &str,
    data: Vec<u8>,
) -> (String, String) {
    let out = uploads::store(db, me, "deal", &d.to_string(), name, data, "nary")
        .await
        .unwrap()
        .unwrap_or_else(|e| panic!("upload: {e}"));
    let id = out["file_id"].as_str().unwrap().to_string();
    let key: String = sqlx::query_scalar("SELECT object_key FROM attachments WHERE id = $1::uuid")
        .bind(&id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    (id, key)
}

fn status_of(e: &granary_core::errors::ApiError) -> u16 {
    match e {
        granary_core::errors::ApiError::Http { status, .. } => status.as_u16(),
        other => panic!("not an HTTP error: {other:?}"),
    }
}

fn meta(title: &str, audiences: &[&str]) -> NewDocument {
    NewDocument {
        title: title.into(),
        category: "products".into(),
        audiences: audiences.iter().map(|s| s.to_string()).collect(),
        ..NewDocument::default()
    }
}

fn details(out: &Value) -> Vec<String> {
    out["proposal"]["details"]
        .as_array()
        .unwrap_or_else(|| panic!("a card: {out}"))
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

/// Applies a card's payload as the action does when the Owner confirms it.
async fn apply(db: &mut PgConnection, actor: &AuthUser, out: &Value) -> Result<Value, String> {
    let action = actions::get(pf::ACTION).unwrap();
    let clean = action.validate(&out["proposal"]["payload"]).unwrap();
    (action.apply)(db, Some(actor), &clean)
        .await
        .map_err(|e| cc::detail(&e))
}

fn customer() -> deliver::Recipient {
    deliver::Recipient {
        company: Some("Vandelay Ltd".into()),
        name: Some("Bob".into()),
        partner_account_id: None,
    }
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_video_version_is_logged_not_watermarked_and_a_pdf_still_stamped() {
    let _serial = SERIAL.lock().await;
    let (pool, store) = setup("libvid_core").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;

    // judged by the bytes: no extension, no type, still an MP4 video (the name gets .mp4)
    let video = mp4(42, 8192);
    let (doc, v) = versions::create_document(
        db,
        &me,
        meta("Laptop demo video", &["customer"]),
        NewFile {
            data: video.clone(),
            file_name: "Laptop demo".into(),
            mime: None,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(v.status, "live");
    assert_eq!(v.file_mime, "video/mp4");
    assert_eq!(v.file_name, "Laptop demo.mp4");
    assert_eq!(v.duration_seconds, Some(42.0));
    assert!(v.extracted_units.is_none(), "no text is read from a video");
    assert_eq!(v.file_bucket, lib::BUCKET_DOCUMENTS);
    assert_eq!(lib::read_file(&v).await.unwrap(), video);
    let shown = lib::view::version_short(&v);
    assert_eq!(shown["duration_seconds"], json!(42.0));

    // a customer copy: its own row and fingerprint, the original bytes, the original's hash
    let before = store.count();
    let sent = deliver::customer_copy_for(
        db,
        &v,
        &customer(),
        "whatsapp",
        deliver::Sender::Staff(me.id),
    )
    .await
    .unwrap();
    assert_eq!(sent.file.data, video, "a video is never watermarked");
    assert_eq!(sent.file.mime, "video/mp4");
    assert!(sent.fingerprint.starts_with("ID-"));
    let (kind, hash, company): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT kind, file_sha256, recipient_company FROM library_downloads WHERE fingerprint = $1",
    )
    .bind(&sent.fingerprint)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(kind, "customer_send");
    assert_eq!(hash.as_deref(), Some(sha(&video).as_str()));
    assert_eq!(company.as_deref(), Some("Vandelay Ltd"));
    assert_eq!(store.count(), before, "nothing new is stored for a copy");
    // the trace finds it by its content and says why there is no mark
    let hit = fingerprint::trace_file(db, &video).await.unwrap().unwrap();
    assert_eq!(hit.document.id, doc.id);
    assert!(!hit.watermarked);
    assert!(
        hit.note
            .as_deref()
            .unwrap_or("")
            .contains("logged, not watermarked"),
        "{:?}",
        hit.note
    );

    // a PDF is still stamped (different bytes, the hash of the stamped copy)
    let original = pdf("Laptop datasheet");
    let (_pdoc, pv) = versions::create_document(
        db,
        &me,
        meta("Laptop datasheet", &["customer"]),
        NewFile {
            data: original.clone(),
            file_name: "datasheet.pdf".into(),
            mime: Some("application/pdf".into()),
        },
        None,
    )
    .await
    .unwrap();
    assert!(pv.duration_seconds.is_none());
    let stamped =
        deliver::customer_copy_for(db, &pv, &customer(), "email", deliver::Sender::Staff(me.id))
            .await
            .unwrap();
    assert_ne!(stamped.file.data, original, "a PDF copy is stamped");
    assert!(stamped.file.data.starts_with(b"%PDF"));
    let hit = fingerprint::trace_file(db, &stamped.file.data)
        .await
        .unwrap()
        .unwrap();
    assert!(hit.watermarked && hit.note.is_none());

    // refused: a file named .mp4 that is a PDF, a text file, a video over the cap
    let e = versions::create_document(
        db,
        &me,
        meta("Fake video", &["internal"]),
        NewFile {
            data: pdf("not a video"),
            file_name: "clip.mp4".into(),
            mime: Some("video/mp4".into()),
        },
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(status_of(&e), 415, "{e:?}");
    assert!(cc::detail(&e).contains("MP4, MOV or WebM"), "{e:?}");
    let e = versions::create_document(
        db,
        &me,
        meta("Notes", &["internal"]),
        NewFile {
            data: b"just some notes".to_vec(),
            file_name: "notes.txt".into(),
            mime: Some("text/plain".into()),
        },
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(status_of(&e), 415, "{e:?}");
    let old = set(db, &[("nary.upload.video_max_mb", "0")]).await;
    let e = versions::create_document(
        db,
        &me,
        meta("Too long", &["internal"]),
        NewFile {
            data: mp4(10, 4096),
            file_name: "long.mp4".into(),
            mime: None,
        },
        None,
    )
    .await
    .unwrap_err();
    restore(db, old).await;
    assert_eq!(status_of(&e), 413, "{e:?}");
    assert!(cc::detail(&e).contains("nary.upload.video_max_mb"), "{e:?}");
}

#[tokio::test]
async fn a_deal_video_becomes_a_product_file_stored_once() {
    let _serial = SERIAL.lock().await;
    let (pool, store) = setup("libvid_card").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let (pid, sku) = priced_product(db, &u("Lane Box"), 5).await;
    let d = deal(db).await;
    let video = mp4(42, 8192);
    let (file_id, key) = attach(db, &me, d, "site walk.mp4", video.clone()).await;
    let objects = store.count();

    let out = run_tool(
        db,
        &me,
        "propose_product_file",
        json!({"product": sku, "file_id": file_id, "title": "Lane Box site walk"}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], pf::ACTION, "{out}");
    assert!(
        out["proposal"].get("auto").is_none(),
        "always a card: {out}"
    );
    let lines = details(&out);
    let all = lines.join("\n");
    for needle in [
        &sku as &str,
        "site walk.mp4 (MP4 video",
        "0:42",
        "the Attachments of deal",
        "Stored once",
        "A new document \"Lane Box site walk\" in Products, audience customer, language EN",
        "logged, never watermarked",
    ] {
        assert!(all.contains(needle), "{needle}: {all}");
    }
    assert_eq!(store.count(), objects, "nothing stored on propose");

    let done = apply(db, &me, &out).await.unwrap();
    assert_eq!(done["status"], "live", "{done}");
    assert_eq!(done["stored_once"], json!(true));
    assert_eq!(store.count(), objects, "one object: the attachment's");
    let did = Uuid::parse_str(done["document_id"].as_str().unwrap()).unwrap();
    let doc = lib::load_document(db, did).await.unwrap().unwrap();
    assert_eq!(doc.audiences, vec!["customer".to_string()]);
    let v = lib::current_live(db, did).await.unwrap().unwrap();
    assert_eq!(v.file_object_key, key);
    assert_eq!(v.file_bucket, lib::BUCKET_ASSETS);
    assert_eq!(v.duration_seconds, Some(42.0));
    assert_eq!(
        v.approved_by,
        Some(me.id),
        "the confirmed card is the approval"
    );
    assert!(lib::references_object(db, &key).await.unwrap());
    // the product's library documents (Sales -> Catalog -> the item) open it
    let found = lib::search(
        db,
        &lib::Scope::Staff(&me),
        None,
        &lib::Filters {
            product_id: Some(pid),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(found.items.iter().any(|x| x.id == did));
    assert_eq!(lib::read_file(&v).await.unwrap(), video);
    let sent = deliver::customer_copy_for(
        db,
        &v,
        &customer(),
        "whatsapp",
        deliver::Sender::Nary {
            confirmed_by: Some(me.id),
        },
    )
    .await
    .unwrap();
    assert_eq!(sent.file.data, video);
    // find_product_files lists it with its length and how it goes
    let files = run_tool(db, &me, "find_product_files", json!({"query": "site walk"})).await;
    let item = files["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["title"] == "Lane Box site walk")
        .unwrap_or_else(|| panic!("listed: {files}"))
        .clone();
    assert_eq!(item["video"], json!(true));
    assert_eq!(item["duration_label"], "0:42");
    assert!(item["sends_as"]
        .as_str()
        .unwrap()
        .contains("not watermarked"));
    assert_eq!(item["file_id"], format!("doc:{did}"));

    // a new version of that document from another deal file, taken off the deal afterwards
    let (pdf_id, pdf_key) = attach(db, &me, d, "site plan.pdf", pdf("Lane Box site plan")).await;
    let out = run_tool(
        db,
        &me,
        "propose_product_file",
        json!({"product": pid.to_string(), "file_id": pdf_id, "document_id": format!("doc:{did}"),
               "remove_from_record": true}),
    )
    .await;
    let all = details(&out).join("\n");
    assert!(
        all.contains("A new version of \"Lane Box site walk\""),
        "{all}"
    );
    assert!(
        all.contains("Then taken off the deal's Attachments"),
        "{all}"
    );
    let done = apply(db, &me, &out).await.unwrap();
    assert_eq!(done["removed_from_record"], json!(true), "{done}");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE id = $1::uuid")
        .bind(&pdf_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(rows, 0, "the attachment row is gone");
    assert!(store.has(&pdf_key), "its object stays");
    let v2 = lib::current_live(db, did).await.unwrap().unwrap();
    assert_eq!(v2.label, "v2");
    assert_eq!(v2.file_object_key, pdf_key);
    assert_eq!(v2.file_mime, "application/pdf");
    // the first deal file is still on the deal
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE id = $1::uuid")
        .bind(&file_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn only_the_owner_and_only_sendable_files() {
    let _serial = SERIAL.lock().await;
    let (pool, store) = setup("libvid_refuse").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let (_pid, sku) = priced_product(db, &u("Lane Box"), 5).await;
    let d = deal(db).await;
    let (file_id, _) = attach(db, &me, d, "demo.mp4", mp4(5, 4096)).await;

    // a catalog editor who is not the Owner: refused at the tool, not listed, refused at apply
    let exec = person(
        db,
        &["sales:view_catalog", "sales:manage_catalog", "crm:view_all"],
    )
    .await;
    let refused = run_tool(
        db,
        &exec,
        "propose_product_file",
        json!({"product": sku, "file_id": file_id}),
    )
    .await;
    assert_eq!(refused["error"], pf::ONLY_OWNER, "{refused}");
    let mut listed = tools_for_user(&exec);
    assert!(listed.iter().any(|t| t.name == "propose_product_file"));
    granary_services::ai::tools::catalog::catalog_change_tools::keep_for_owner(
        db,
        &exec,
        &mut listed,
    )
    .await
    .unwrap();
    assert!(!listed.iter().any(|t| t.name == "propose_product_file"));
    let out = run_tool(
        db,
        &me,
        "propose_product_file",
        json!({"product": sku, "file_id": file_id}),
    )
    .await;
    assert_eq!(out["proposal"]["payload"]["title"], "demo", "{out}");
    assert_eq!(out["proposal"]["payload"]["audience"], "customer");
    assert_eq!(apply(db, &exec, &out).await.unwrap_err(), pf::ONLY_OWNER);

    // a wrong type is refused on propose (a text file on the deal)
    let key = format!("attachments/crm_deal/{d}/{}/notes.txt", Uuid::new_v4());
    store
        .put_object(
            &granary_core::config::settings().MINIO_BUCKET_ASSETS,
            &key,
            b"plain notes",
            "text/plain",
        )
        .await
        .unwrap();
    let text_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, uploaded_by_id, file_name, file_size, mime_type, \
         object_key, url, file_type) VALUES ($1, 'crm_deal', $2, $3, 'notes.txt', 11, 'text/plain', $4, '', 'file')",
    )
    .bind(text_id)
    .bind(d.to_string())
    .bind(me.id)
    .bind(&key)
    .execute(&mut *db)
    .await
    .unwrap();
    let wrong = run_tool(
        db,
        &me,
        "propose_product_file",
        json!({"product": sku, "file_id": text_id.to_string()}),
    )
    .await;
    assert!(
        wrong["error"]
            .as_str()
            .unwrap_or("")
            .contains("must be a PDF, an image"),
        "{wrong}"
    );
    // a bad audience, an unknown product, remove_from_record on a file that is not an attachment
    let bad = run_tool(
        db,
        &me,
        "propose_product_file",
        json!({"product": sku, "file_id": file_id, "audience": "everyone"}),
    )
    .await;
    assert!(bad["error"].as_str().unwrap().contains("audience"), "{bad}");
    let bad = run_tool(
        db,
        &me,
        "propose_product_file",
        json!({"product": "NO-SUCH-SKU-XYZ", "file_id": file_id}),
    )
    .await;
    assert!(bad["error"].is_string(), "{bad}");

    // Ask, never promoted, confirmed twice; the chat's confirm runs it
    assert!(autonomy::never_promote(db)
        .await
        .unwrap()
        .contains(&pf::ACTION.to_string()));
    assert!(autonomy::confirm_twice(db)
        .await
        .unwrap()
        .contains(&pf::ACTION.to_string()));
    assert_eq!(
        autonomy::level_for(db, pf::ACTION).await.unwrap(),
        autonomy::ASK
    );
    assert!(autonomy::set_level(db, pf::ACTION, 3, Some(&me))
        .await
        .is_err());
    assert!(granary_services::nary::executor::CHAT_ACTIONS.contains(&pf::ACTION));
    assert!(granary_services::ai::mcp_access::CATALOG_ACTIONS.contains(&pf::ACTION));
    assert!(granary_services::ai::mcp_access::PROPOSAL_TOOLS.contains(&"propose_product_file"));
}
