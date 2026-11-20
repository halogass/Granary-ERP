//! routers/attachments.py through the real app, with a fake object store (Python monkeypatches
//! `_minio_client`). Ports the endpoint tests of backend/tests/test_attachment_access.py:
//! - test_the_download_endpoint_refuses_a_file_on_someone_elses_deal
//! - test_the_list_endpoint_still_works_for_the_owner
//! - test_the_attachment_response_no_longer_leaks_the_raw_minio_url
//!
//! Plus upload / download / delete through the store (no Python test). The rule tests of that file call
//! `assert_can_access` directly and are services tests. Validation and the 404s are in the HTTP suite
//! (compat-tests/http-suite/tests/test_attachments_routes.py).

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use granary_services::storage::{ObjectStore, StoreError};
use reqwest::multipart::{Form, Part};
use support::{app, TestApp};
use uuid::Uuid;

const TAG: &str = "attachments_routes";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct FakeStore(Mutex<HashMap<String, (Vec<u8>, String)>>);

impl ObjectStore for FakeStore {
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
        content_type: &'f str,
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
            self.0
                .lock()
                .unwrap()
                .insert(key.to_string(), (data.to_vec(), content_type.to_string()));
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
            self.0
                .lock()
                .unwrap()
                .get(key)
                .map(|(d, _)| d.clone())
                .ok_or_else(|| StoreError {
                    kind: "S3Error".into(),
                    message: format!("NoSuchKey {key}"),
                })
        })
    }
    fn remove_object<'a, 'b, 'd, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
    ) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.0.lock().unwrap().remove(key);
            Ok(())
        })
    }
}

fn store() -> Arc<FakeStore> {
    static STORE: OnceLock<Arc<FakeStore>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let s = Arc::new(FakeStore::default());
            granary_services::storage::set_store(Some(s.clone() as Arc<dyn ObjectStore>));
            s
        })
        .clone()
}

/// `_deal(owner_id)`.
async fn deal(app: &TestApp, owner: Uuid) -> Uuid {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, assigned_to_id, created_by_id, custom, is_deleted) \
         VALUES ($1, $2, 'qualifying', $3, $3, '{}', false)",
    )
    .bind(id)
    .bind(support::u("pytest"))
    .bind(owner)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

async fn attachment_row(
    app: &TestApp,
    deal_id: Uuid,
    owner: Uuid,
    key: &str,
    mime: &str,
    size: i32,
) -> Uuid {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, uploaded_by_id, file_name, file_size, mime_type, \
         object_key, url, file_type) VALUES ($1, 'crm_deal', $2, $3, 'secret.pdf', $4, $5, $6, '', 'file')",
    )
    .bind(id)
    .bind(deal_id.to_string())
    .bind(owner)
    .bind(size)
    .bind(mime)
    .bind(key)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_the_download_endpoint_refuses_a_file_on_someone_elses_deal() {
    let app = app(TAG);
    let owner = app.user_with(&["crm:view_own"]).await;
    let intruder = app.user_with(&["crm:view_own"]).await;
    let d = deal(app, owner.id).await;
    let att = attachment_row(
        app,
        d,
        owner.id,
        "attachments/crm_deal/x/secret.pdf",
        "application/pdf",
        1,
    )
    .await;
    let r = app
        .get(
            &intruder.token,
            &format!("/api/v1/attachments/{att}/download"),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .get(
            &intruder.token,
            &format!("/api/v1/attachments?entity_type=crm_deal&entity_id={d}"),
        )
        .await;
    assert_eq!(r.status, 403);
    // the intruder may not delete it either
    let r = app
        .delete(&intruder.token, &format!("/api/v1/attachments/{att}"))
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn test_the_list_endpoint_still_works_for_the_owner() {
    let app = app(TAG);
    let owner = app.user_with(&["crm:view_own"]).await;
    let d = deal(app, owner.id).await;
    let r = app
        .get(
            &owner.token,
            &format!("/api/v1/attachments?entity_type=crm_deal&entity_id={d}"),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), serde_json::json!([]));
}

#[tokio::test]
async fn test_the_attachment_response_no_longer_leaks_the_raw_minio_url() {
    let row = granary_api::routers::attachments::AttachmentRow {
        id: Uuid::new_v4(),
        entity_type: "crm_deal".into(),
        entity_id: Uuid::new_v4().to_string(),
        file_name: "x.pdf".into(),
        file_size: 1,
        mime_type: "application/pdf".into(),
        object_key: "k".into(),
        file_type: "file".into(),
        uploaded_by_id: None,
        created_at: chrono::Utc::now(),
        uploader_name: None,
        uploaded_via: None,
    };
    let out = granary_api::routers::attachments::attachment_dict(&row, Some("Someone"));
    assert!(out.get("url").is_none());
    assert_eq!(out["file_name"], "x.pdf");
    assert_eq!(out["uploaded_by_name"], "Someone");
}

fn upload_form(entity_id: &str, name: &str, mime: &str, data: Vec<u8>) -> Form {
    Form::new()
        .percent_encode_noop()
        .text("entity_type", "crm_deal")
        .text("entity_id", entity_id.to_string())
        .part(
            "file",
            Part::bytes(data)
                .file_name(name.to_string())
                .mime_str(mime)
                .unwrap(),
        )
}

#[tokio::test]
async fn upload_download_list_and_delete_through_the_store() {
    let app = app(TAG);
    let s = store();
    let owner = app
        .make_user(
            support::UserSpec::new()
                .perms(&["crm:view_own"])
                .name("Rachel Owner"),
        )
        .await;
    let d = deal(app, owner.id).await;

    let r = app
        .send(
            app.client()
                .post(app.url("/api/v1/attachments/upload"))
                .bearer_auth(&owner.token)
                .multipart(upload_form(
                    &d.to_string(),
                    "dir/My report é.pdf",
                    "application/pdf",
                    b"%PDF-1".to_vec(),
                )),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let body = r.json();
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "entity_type",
            "entity_id",
            "file_name",
            "file_size",
            "mime_type",
            "file_type",
            "uploaded_by_id",
            "uploaded_by_name",
            "created_at"
        ]
    );
    assert_eq!(body["file_name"], "My report é.pdf");
    assert_eq!(body["file_size"], 6);
    assert_eq!(body["file_type"], "file");
    assert_eq!(body["uploaded_by_name"], "Rachel Owner");
    let id = body["id"].as_str().unwrap().to_string();
    let mut db = app.db().await;
    let (key, url): (String, String) =
        sqlx::query_as("SELECT object_key, url FROM attachments WHERE id = $1")
            .bind(Uuid::parse_str(&id).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(key.starts_with(&format!("attachments/crm_deal/{d}/")));
    assert!(key.ends_with("/My report é.pdf"));
    // security 26 Oct: the bucket is private, no permanent link is kept
    assert_eq!(url, "");
    assert_eq!(
        s.0.lock().unwrap().get(&key).cloned(),
        Some((b"%PDF-1".to_vec(), "application/pdf".to_string()))
    );

    let r = app
        .get(&owner.token, &format!("/api/v1/attachments/{id}/download"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"%PDF-1");
    assert_eq!(r.header("content-type").as_deref(), Some("application/pdf"));
    assert_eq!(
        r.header("content-disposition").as_deref(),
        Some("inline; filename=\"My report _.pdf\"; filename*=UTF-8''My%20report%20%C3%A9.pdf")
    );
    assert_eq!(
        r.header("cache-control").as_deref(),
        Some("private, max-age=300")
    );
    assert_eq!(
        r.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );
    assert_eq!(r.header("content-length").as_deref(), Some("6"));

    let r = app
        .get(
            &owner.token,
            &format!("/api/v1/attachments?entity_type=crm_deal&entity_id={d}"),
        )
        .await;
    let list = r.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["uploaded_by_name"], "Rachel Owner");
    assert_eq!(list[0], body);

    // someone else with crm:view_all may see it but not delete it
    let viewer = app.user_with(&["crm:view_all"]).await;
    let r = app
        .delete(&viewer.token, &format!("/api/v1/attachments/{id}"))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json(),
        serde_json::json!({"detail": "You can only delete your own attachments"})
    );

    let r = app
        .delete(&owner.token, &format!("/api/v1/attachments/{id}"))
        .await;
    assert_eq!(r.status, 204);
    assert!(s.0.lock().unwrap().get(&key).is_none());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE id = $1")
        .bind(Uuid::parse_str(&id).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn upload_refuses_types_and_sizes_and_downloads_svg_as_attachment() {
    let app = app(TAG);
    store();
    let sa = app.superadmin().await;
    let owner = app.user_with(&["crm:view_own"]).await;
    let d = deal(app, owner.id).await;
    let send = |form: Form| {
        app.send(
            app.client()
                .post(app.url("/api/v1/attachments/upload"))
                .bearer_auth(&sa.token)
                .multipart(form),
        )
    };
    let r = send(upload_form(
        &d.to_string(),
        "x.svg",
        "image/svg+xml",
        b"<svg/>".to_vec(),
    ))
    .await;
    assert_eq!(r.status, 415);
    assert_eq!(
        r.json(),
        serde_json::json!({"detail": "Files of type 'image/svg+xml' can't be uploaded here."})
    );
    let r = send(upload_form(
        &d.to_string(),
        "x.png",
        "image/png",
        vec![0; 5 * 1024 * 1024 + 1],
    ))
    .await;
    assert_eq!(r.status, 413);
    assert_eq!(
        r.json(),
        serde_json::json!({"detail": "Image exceeds 5 MB limit"})
    );
    let r = send(upload_form(
        &d.to_string(),
        "x.zip",
        "application/zip",
        vec![0; 25 * 1024 * 1024 + 1],
    ))
    .await;
    assert_eq!(r.status, 413);
    assert_eq!(
        r.json(),
        serde_json::json!({"detail": "Document exceeds 25 MB limit"})
    );
    let r = send(upload_form(
        &d.to_string(),
        "a.png",
        "image/png",
        vec![1, 2, 3],
    ))
    .await;
    assert_eq!(r.status, 201);
    assert_eq!(r.json()["file_type"], "image");

    // an old row carrying a type no longer accepted downloads as an attachment, not inline
    let key = format!("attachments/crm_deal/{d}/old.svg");
    store()
        .0
        .lock()
        .unwrap()
        .insert(key.clone(), (b"<svg/>".to_vec(), "image/svg+xml".into()));
    let att = attachment_row(app, d, owner.id, &key, "image/svg+xml", 0).await;
    let r = app
        .get(&sa.token, &format!("/api/v1/attachments/{att}/download"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("content-disposition").as_deref(),
        Some("attachment; filename=\"secret.pdf\"; filename*=UTF-8''secret.pdf")
    );
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(
        r.header("content-security-policy").as_deref(),
        Some("default-src 'none'; sandbox")
    );

    // security 26 Oct: an HTML file (e.g. a Nary customer file keeping the sender's type) never opens as a page
    let key = format!("attachments/crm_deal/{d}/page.html");
    store().0.lock().unwrap().insert(
        key.clone(),
        (b"<script>alert(1)</script>".to_vec(), "text/html".into()),
    );
    let att = attachment_row(app, d, owner.id, &key, "text/html", 0).await;
    let r = app
        .get(&sa.token, &format!("/api/v1/attachments/{att}/download"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/octet-stream")
    );
    assert!(r
        .header("content-disposition")
        .unwrap()
        .starts_with("attachment;"));
    assert_eq!(
        r.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );
    assert_eq!(
        r.header("content-security-policy").as_deref(),
        Some("default-src 'none'; sandbox")
    );
    // missing object
    let gone = attachment_row(
        app,
        d,
        owner.id,
        "attachments/nowhere",
        "application/pdf",
        3,
    )
    .await;
    let r = app
        .get(&sa.token, &format!("/api/v1/attachments/{gone}/download"))
        .await;
    assert_eq!(r.status, 404);
    assert_eq!(
        r.json(),
        serde_json::json!({"detail": "File not found in storage"})
    );
}
