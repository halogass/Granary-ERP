//! routers/email_signature.py through the real app: a stored asset served from a fake object store
//! (MinIO is unreachable in the HTTP suite). The other routes are pinned on both stacks by
//! compat-tests/http-suite/tests/test_email_signature_routes.py.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use granary_services::storage::{ObjectStore, StoreError};
use support::app;
use uuid::Uuid;

const TAG: &str = "email_signature_routes";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

struct OneObject;

impl ObjectStore for OneObject {
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
        _key: &'d str,
        _data: &'e [u8],
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
        Box::pin(async { Ok(()) })
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
            if key == "signature/logo.png" {
                Ok(b"\x89PNG".to_vec())
            } else {
                Err(StoreError {
                    kind: "S3Error".into(),
                    message: "NoSuchKey".into(),
                })
            }
        })
    }
}

#[tokio::test]
async fn a_stored_asset_is_served_publicly_with_its_type_and_cache_header() {
    let app = app(TAG);
    granary_services::storage::set_store(Some(Arc::new(OneObject) as Arc<dyn ObjectStore>));
    let mut db = app.db().await;
    let (logo, no_mime, gone) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    for (id, mime, key) in [
        (logo, "image/gif", "signature/logo.png"),
        (no_mime, "", "signature/logo.png"),
        (gone, "image/png", "signature/gone.png"),
    ] {
        sqlx::query(
            "INSERT INTO email_signature_assets (id, filename, mime_type, object_key, file_size) \
             VALUES ($1, 'logo.png', $2, $3, 4)",
        )
        .bind(id)
        .bind(mime)
        .bind(key)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = app
        .get("", &format!("/api/v1/email/signature/assets/{logo}"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"\x89PNG");
    assert_eq!(r.header("content-type").as_deref(), Some("image/gif"));
    assert_eq!(
        r.header("cache-control").as_deref(),
        Some("public, max-age=86400")
    );
    let r = app
        .get("", &format!("/api/v1/email/signature/assets/{no_mime}"))
        .await;
    assert_eq!(r.header("content-type").as_deref(), Some("image/png"));
    let r = app
        .get("", &format!("/api/v1/email/signature/assets/{gone}"))
        .await;
    assert_eq!(r.status, 404);
    assert_eq!(r.text(), r#"{"detail":"Asset file missing from storage"}"#);
}
