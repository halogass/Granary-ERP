//! doclib A2: the copy endpoints (view, download, trace, partner portal) through the real app.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use granary_services::pdftools::{Document, Point, SaveOptions, TextOptions};
use granary_services::storage::{self, ObjectStore, StoreError};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use support::{u, UserSpec};
use uuid::Uuid;

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl ObjectStore for MemStore {
    fn bucket_exists<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<bool, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(true) })
    }
    fn make_bucket<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(()) })
    }
    fn put_object<'a, 'b, 'd, 'e, 'f, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
        data: &'e [u8],
        _content_type: &'f str,
    ) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        'e: 'c,
        'f: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.blobs
                .lock()
                .unwrap()
                .insert(key.to_string(), data.to_vec());
            Ok(())
        })
    }
    fn get_object<'a, 'b, 'd, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
    ) -> Fut<'c, Result<Vec<u8>, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.blobs
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or(StoreError {
                    kind: "S3Error".into(),
                    message: key.into(),
                })
        })
    }
}

fn store() -> Arc<MemStore> {
    static S: OnceLock<Arc<MemStore>> = OnceLock::new();
    S.get_or_init(|| {
        let s = Arc::new(MemStore::default());
        storage::set_store(Some(s.clone()));
        s
    })
    .clone()
}

fn app() -> &'static support::TestApp {
    store();
    support::app("doclib_copies")
}

fn pdf(label: &str) -> Vec<u8> {
    let mut d = Document::new().unwrap();
    let p = d.new_page(595.0, 842.0).unwrap();
    d.insert_text(p, Point::new(72.0, 100.0), label, &TextOptions::default())
        .unwrap();
    d.tobytes(SaveOptions {
        garbage: 1,
        deflate: true,
        clean: false,
    })
    .unwrap()
}

/// A document with a live v1 and a superseded v0; (document id, live version id, old version id).
async fn document(audiences: &[&str]) -> (Uuid, Uuid, Uuid) {
    let app = app();
    let mut db = app.db().await;
    let (did, live, old) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let cat: Uuid = sqlx::query_scalar("SELECT id FROM doc_categories WHERE key = 'company'")
        .fetch_one(&mut db)
        .await
        .unwrap();
    let auds: Vec<String> = audiences.iter().map(|s| s.to_string()).collect();
    sqlx::query(
        "INSERT INTO library_documents (id, title, category_id, audiences, language, kind) VALUES ($1, $2, $3, $4, 'en', 'uploaded')",
    )
    .bind(did)
    .bind(u("Company profile"))
    .bind(cat)
    .bind(&auds)
    .execute(&mut db)
    .await
    .unwrap();
    for (vid, label, status) in [(old, "v0", "superseded"), (live, "v1", "live")] {
        let key = format!("doclib/apitest/{vid}");
        store()
            .blobs
            .lock()
            .unwrap()
            .insert(key.clone(), pdf(&format!("Profile {label}")));
        sqlx::query(
            "INSERT INTO library_document_versions (id, document_id, label, change_note, status, file_object_key, \
             file_name, file_mime, file_size, file_sha256) VALUES ($1, $2, $3, 'note', $4, $5, 'profile.pdf', \
             'application/pdf', 1, 'x')",
        )
        .bind(vid)
        .bind(did)
        .bind(label)
        .bind(status)
        .bind(&key)
        .execute(&mut db)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE library_documents SET current_version_id = $2 WHERE id = $1")
        .bind(did)
        .bind(live)
        .execute(&mut db)
        .await
        .unwrap();
    (did, live, old)
}

fn text_of(bytes: &[u8]) -> String {
    Document::open(bytes).unwrap().get_text(0).unwrap()
}

#[tokio::test]
async fn view_and_download_are_stamped_and_logged() {
    let app = app();
    let (did, _live, old) = document(&["internal"]).await;
    let staff = app
        .make_user(
            UserSpec::new()
                .perms(&["documents:view"])
                .name("Rachel Staff"),
        )
        .await;
    let r = app
        .get(
            &staff.token,
            &format!("/api/v1/knowledge/documents/{did}/view"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("content-type").as_deref(), Some("application/pdf"));
    assert!(r
        .header("content-disposition")
        .unwrap()
        .starts_with("inline"));
    let fp = r
        .header("x-document-fingerprint")
        .expect("fingerprint header");
    let t = text_of(&r.body);
    assert!(t.contains("Profile v1"), "{t}");
    assert!(
        t.contains("Downloaded by Rachel Staff · ") && t.contains(&fp),
        "{t}"
    );

    let r = app
        .get(
            &staff.token,
            &format!("/api/v1/knowledge/documents/{did}/download"),
        )
        .await;
    assert_eq!(r.status, 200);
    assert!(r
        .header("content-disposition")
        .unwrap()
        .starts_with("attachment"));
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM library_downloads WHERE document_id = $1 AND user_id = $2 \
         AND kind = 'staff_download' AND channel = 'web' AND file_sha256 IS NOT NULL",
    )
    .bind(did)
    .bind(staff.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 2);

    // the editable source: editors only; old versions: doc admins only
    let r = app
        .get(
            &staff.token,
            &format!("/api/v1/knowledge/documents/{did}/download?source=1"),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .get(
            &staff.token,
            &format!("/api/v1/knowledge/documents/{did}/view?version={old}"),
        )
        .await;
    assert_eq!(r.status, 403);
    let admin = app
        .make_user(UserSpec::new().perms(&["documents:view", "documents:admin"]))
        .await;
    let r = app
        .get(
            &admin.token,
            &format!("/api/v1/knowledge/documents/{did}/view?version={old}"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(text_of(&r.body).contains("Profile v0"));
    // someone without documents:view
    let nobody = app.user_with(&["crm:view"]).await;
    let r = app
        .get(
            &nobody.token,
            &format!("/api/v1/knowledge/documents/{did}/view"),
        )
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn trace_is_for_doc_admins_by_file_or_id() {
    let app = app();
    let (did, _, _) = document(&["internal"]).await;
    let staff = app
        .make_user(
            UserSpec::new()
                .perms(&["documents:view"])
                .name("Leaky Larry"),
        )
        .await;
    let copy = app
        .get(
            &staff.token,
            &format!("/api/v1/knowledge/documents/{did}/download"),
        )
        .await;
    assert_eq!(copy.status, 200);
    let fp = copy.header("x-document-fingerprint").unwrap();

    let r = app
        .post(
            &staff.token,
            "/api/v1/knowledge/documents/trace",
            json!({"fingerprint": fp}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());

    let admin = app
        .make_user(UserSpec::new().perms(&["documents:view", "documents:admin"]))
        .await;
    let form = Form::new().part(
        "file",
        Part::bytes(copy.body.clone())
            .file_name("leak.pdf")
            .mime_str("application/pdf")
            .unwrap(),
    );
    let r = app
        .send(
            app.client()
                .post(app.url("/api/v1/knowledge/documents/trace"))
                .bearer_auth(&admin.token)
                .multipart(form),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let hit = r.json();
    assert_eq!(hit["fingerprint"], fp.as_str());
    assert_eq!(hit["matched_by"], "fingerprint");
    assert_eq!(hit["user_name"], "Leaky Larry");
    assert_eq!(hit["document_id"], did.to_string());
    assert_eq!(hit["version_label"], "v1");
    assert_eq!(hit["kind"], "staff_download");

    let r = app
        .post(
            &admin.token,
            "/api/v1/knowledge/documents/trace",
            json!({"fingerprint": fp.to_lowercase()}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["matched_by"], "id");
    let r = app
        .post(
            &admin.token,
            "/api/v1/knowledge/documents/trace",
            json!({"fingerprint": "ID-ZZZZZZZZZZ"}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), Value::Null);
    let r = app
        .post(&admin.token, "/api/v1/knowledge/documents/trace", json!({}))
        .await;
    assert_eq!(r.status, 422);
}

#[tokio::test]
async fn partner_portal_lists_and_stamps_partner_documents() {
    let app = app();
    let (pdoc, _, _) = document(&["partner"]).await;
    let (cdoc, _, _) = document(&["customer"]).await;
    let company = u("Partner Ltd");
    let mut db = app.db().await;
    let pid = Uuid::new_v4();
    sqlx::query("INSERT INTO partner_accounts (id, company_name, partner_type, is_deleted) VALUES ($1, $2, 'reseller', false)")
        .bind(pid)
        .bind(&company)
        .execute(&mut db)
        .await
        .unwrap();
    let portal = app.make_user(UserSpec::new().portal("partner")).await;
    sqlx::query("UPDATE users SET partner_account_id = $2 WHERE id = $1")
        .bind(portal.id)
        .bind(pid)
        .execute(&mut db)
        .await
        .unwrap();

    let r = app.get(&portal.token, "/api/v1/partner/documents").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&pdoc.to_string()));
    assert!(!ids.contains(&cdoc.to_string()));
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == pdoc.to_string())
        .unwrap();
    assert_eq!(item["owner"], Value::Null);
    assert_eq!(item["current_version"]["label"], "v1");

    let r = app
        .get(
            &portal.token,
            &format!("/api/v1/partner/documents/{pdoc}/download"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let t = text_of(&r.body);
    assert!(t.contains(&format!("Prepared for {company} · ")), "{t}");
    let r = app
        .get(
            &portal.token,
            &format!("/api/v1/partner/documents/{cdoc}/download"),
        )
        .await;
    assert_eq!(r.status, 404);
    let kind: String = sqlx::query_scalar(
        "SELECT kind FROM library_downloads WHERE document_id = $1 AND partner_account_id = $2",
    )
    .bind(pdoc)
    .bind(pid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(kind, "partner_download");

    // staff are not partners; partners are not staff
    let staff = app.user_with(&["documents:view"]).await;
    assert_eq!(
        app.get(&staff.token, "/api/v1/partner/documents")
            .await
            .status,
        403
    );
    let r = app
        .get(
            &portal.token,
            &format!("/api/v1/knowledge/documents/{pdoc}/view"),
        )
        .await;
    assert_eq!(r.status, 403);
}
