//! security 26 Oct: browser links to private objects are short presigned links signed for the host the browser
//! reaches MinIO at; the public bucket is the only one with permanent links.

mod support;

use granary_services::storage::{self, presign_get_url};

const PUBLIC: &str = "https://erp.example/minio";

fn init() {
    support::init();
    // settings() is read once per binary: every test here sets the same values first.
    std::env::set_var("MINIO_PUBLIC_URL", PUBLIC);
    std::env::set_var("MINIO_ACCESS_KEY", "granary");
    std::env::set_var("MINIO_SECRET_KEY", "test-minio-secret");
    std::env::set_var("MINIO_ENDPOINT", "minio:9000");
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgresql+asyncpg://granary:granarytest@127.0.0.1:55433/unused",
        );
    }
}

fn query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    url.split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
}

#[test]
fn browser_links_are_signed_for_the_public_host_without_the_proxy_prefix() {
    init();
    let link =
        storage::browser_link("granary-assets", "messaging/u/a b.png", 900, None, None).unwrap();
    assert!(
        link.starts_with(&format!(
            "{PUBLIC}/granary-assets/messaging/u/a%20b.png?X-Amz-Algorithm="
        )),
        "{link}"
    );
    assert_eq!(query_param(&link, "X-Amz-Expires"), Some("900"));
    // the same signature MinIO computes for GET /granary-assets/... with Host erp.example (nginx strips /minio)
    let date = query_param(&link, "X-Amz-Date").unwrap();
    let now = chrono::NaiveDateTime::parse_from_str(date, "%Y%m%dT%H%M%SZ")
        .unwrap()
        .and_utc();
    let direct = presign_get_url(
        "erp.example",
        true,
        "granary",
        "test-minio-secret",
        "granary-assets",
        "messaging/u/a b.png",
        900,
        now,
    )
    .unwrap();
    assert_eq!(
        link,
        direct.replacen("https://erp.example/", &format!("{PUBLIC}/"), 1)
    );
    assert!(storage::browser_link("granary-assets", "k", 0, None, None).is_err());
}

#[test]
fn browser_links_force_a_download_for_scriptable_types() {
    init();
    let html = storage::browser_link(
        "granary-assets",
        "attachments/x/page.html",
        900,
        Some("text/html"),
        Some("page.html"),
    )
    .unwrap();
    assert_eq!(
        query_param(&html, "response-content-type"),
        Some("application%2Foctet-stream")
    );
    assert!(query_param(&html, "response-content-disposition")
        .unwrap()
        .starts_with("attachment%3B%20filename%3D%22page.html%22"));
    // the overrides are signed: they come before the signature
    assert!(html.find("response-content-type").unwrap() < html.find("X-Amz-Signature").unwrap());
    let png =
        storage::browser_link("granary-assets", "a.png", 900, Some("image/png"), None).unwrap();
    assert!(!png.contains("response-content"));
}

#[test]
fn messaging_keys_come_from_keys_old_links_and_short_links_only() {
    init();
    let key = "messaging/0b6f1c56-3f0e-4c45-9d43-2f7a3f0a9b11/site photo.png";
    assert_eq!(storage::messaging_key(key).as_deref(), Some(key));
    assert_eq!(
        storage::messaging_key(&format!("{PUBLIC}/granary-assets/{key}")).as_deref(),
        Some(key)
    );
    let link = storage::browser_link("granary-assets", key, 900, None, None).unwrap();
    assert_eq!(storage::messaging_key(&link).as_deref(), Some(key));
    for other in [
        "/api/v1/attachments/x/download",
        "https://evil.example/granary-assets/messaging/x/y",
        "https://erp.example/minio/granary-assets/attachments/x/y.pdf",
        "https://erp.example/minio/granary-documents/messaging/x/y.pdf",
        "messaging/../attachments/x",
        "messaging/",
    ] {
        assert_eq!(storage::messaging_key(other), None, "{other}");
    }
    let shown = storage::message_file_link(Some(key.to_string()), 600).unwrap();
    assert!(shown.starts_with(&format!("{PUBLIC}/granary-assets/messaging/")));
    assert_eq!(query_param(&shown, "X-Amz-Expires"), Some("600"));
    assert_eq!(
        storage::message_file_link(Some("/api/v1/x".into()), 600).as_deref(),
        Some("/api/v1/x")
    );
    assert_eq!(storage::message_file_link(None, 600), None);
}

#[test]
fn public_objects_live_in_the_public_bucket() {
    init();
    assert_eq!(storage::public_bucket(), "granary-public");
    assert_eq!(
        storage::public_object_url("branding/logo.png"),
        format!("{PUBLIC}/granary-public/branding/logo.png")
    );
    assert_ne!(
        storage::public_bucket(),
        granary_core::config::settings().MINIO_BUCKET_ASSETS
    );
}
