//! security 26 Oct: private file storage and safe file responses.
//! - the shared `safe_file_response` never serves a scriptable type inline
//! - a chat file is stored by key and every response carries a fresh short link (not a permanent URL)
//!
//! An HTML record attachment downloading as octet-stream is tested in attachments_routes.rs.

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use granary_services::storage::{ObjectStore, StoreError};
use reqwest::multipart::{Form, Part};
use serde_json::json;
use support::TestApp;
use uuid::Uuid;

const TAG: &str = "private_storage_routes";
const PUBLIC: &str = "https://erp.example/minio";

fn app() -> &'static TestApp {
    std::env::set_var("MINIO_PUBLIC_URL", PUBLIC);
    support::app(TAG)
}

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

fn header(r: &axum::response::Response, name: &str) -> String {
    r.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

#[test]
fn safe_file_response_never_serves_a_page() {
    for (mime, name) in [
        ("image/svg+xml", "logo.svg"),
        ("text/html", "invoice.html"),
        ("application/xhtml+xml", "x.xhtml"),
        ("text/xml", "x.xml"),
        ("application/javascript", "x.js"),
    ] {
        let r = granary_api::files::safe_file_response(b"<x/>".to_vec(), mime, name, true);
        assert_eq!(
            header(&r, "content-type"),
            "application/octet-stream",
            "{mime}"
        );
        assert!(
            header(&r, "content-disposition").starts_with("attachment; filename=\""),
            "{mime}"
        );
        assert_eq!(header(&r, "x-content-type-options"), "nosniff");
        assert_eq!(
            header(&r, "content-security-policy"),
            "default-src 'none'; sandbox"
        );
    }
    let r = granary_api::files::safe_file_response(b"x".to_vec(), "image/png", "a b.png", true);
    assert_eq!(header(&r, "content-type"), "image/png");
    assert_eq!(
        header(&r, "content-disposition"),
        "inline; filename=\"a b.png\"; filename*=UTF-8''a%20b.png"
    );
    assert_eq!(
        header(&r, "content-security-policy"),
        "default-src 'none'; sandbox"
    );
    let r = granary_api::files::safe_file_response(b"x".to_vec(), "text/plain", "", false);
    assert_eq!(header(&r, "content-type"), "text/plain; charset=utf-8");
    assert!(header(&r, "content-disposition").starts_with("attachment; filename=\"download\""));
}

fn assert_short_link(url: &str, key_part: &str, secs: i64) {
    assert!(
        url.starts_with(&format!("{PUBLIC}/granary-assets/messaging/")),
        "{url}"
    );
    assert!(url.contains(key_part), "{url}");
    assert!(url.contains(&format!("X-Amz-Expires={secs}&")), "{url}");
    assert!(url.contains("&X-Amz-Signature="), "{url}");
}

#[tokio::test]
async fn chat_files_are_kept_by_key_and_returned_as_short_links() {
    let app = app();
    let _g = app.serial().await;
    let s = store();
    app.set_setting("storage.link_minutes", "15").await;
    let alice = app.user_with(&[]).await;
    let bob = app.user_with(&[]).await;

    let r = app
        .send(
            app.client()
                .post(app.url("/api/v1/messaging/upload"))
                .bearer_auth(&alice.token)
                .multipart(
                    Form::new().percent_encode_noop().part(
                        "file",
                        Part::bytes(b"png".to_vec())
                            .file_name("site photo.png")
                            .mime_str("image/png")
                            .unwrap(),
                    ),
                ),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let up = r.json();
    let link = up["url"].as_str().unwrap().to_string();
    assert_short_link(&link, "/site%20photo.png?", 900);
    let key =
        s.0.lock()
            .unwrap()
            .keys()
            .find(|k| k.starts_with("messaging/") && k.ends_with("/site photo.png"))
            .cloned()
            .unwrap();

    // the client sends the link back; the message keeps the key
    let r = app
        .post(
            &alice.token,
            "/api/v1/messaging/send",
            json!({"receiver_id": bob.id.to_string(), "content": "", "message_type": "image",
                   "file_url": link, "file_name": "site photo.png", "mime_type": "image/png"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let mut db = app.db().await;
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT file_url FROM direct_messages WHERE sender_id = $1 AND receiver_id = $2",
    )
    .bind(alice.id)
    .bind(bob.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(stored.as_deref(), Some(key.as_str()));

    // an old permanent link (before the bucket went private) is kept as a key too
    let old_key = format!("messaging/{}/old.pdf", Uuid::new_v4());
    let r = app
        .post(
            &alice.token,
            "/api/v1/messaging/send",
            json!({"receiver_id": bob.id.to_string(), "content": "", "message_type": "file",
                   "file_url": format!("{PUBLIC}/granary-assets/{old_key}")}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    // anything else is refused
    for bad in [
        "https://evil.example/x.png".to_string(),
        format!("{PUBLIC}/granary-assets/attachments/crm_deal/x/secret.pdf"),
        format!("{PUBLIC}/granary-documents/messaging/{}/x", Uuid::new_v4()),
        "messaging/../attachments/x".to_string(),
    ] {
        let r = app
            .post(
                &alice.token,
                "/api/v1/messaging/send",
                json!({"receiver_id": bob.id.to_string(), "content": "x", "file_url": bad}),
            )
            .await;
        assert_eq!(r.status, 422, "{bad}: {}", r.text());
    }

    // the record comes back with a fresh link, its lifetime from storage.link_minutes
    app.set_setting("storage.link_minutes", "5").await;
    let r = app
        .get(
            &bob.token,
            &format!("/api/v1/messaging/messages/{}", alice.id),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let msgs = r.json();
    let urls: Vec<&str> = msgs
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["file_url"].as_str())
        .collect();
    assert_eq!(urls.len(), 2, "{msgs}");
    for u in &urls {
        assert_short_link(u, "messaging/", 300);
    }
    assert!(urls.iter().any(|u| u.contains("/old.pdf?")));
    app.set_setting("storage.link_minutes", "15").await;
}
