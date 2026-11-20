//! Nary batch 4 (agent D1) through the real app: POST /nary/chat-files, the web chat's paperclip.
//!
//! - A PDF (any file Nary reads) is stored for the sender and answered with the chat line naming its file_id
//!   for read_attachment; the same file again is the same file_id.
//! - A notes file from the Owner still goes to the meeting files (attach_meeting_notes); from someone without
//!   nary:approve it is a plain file.

#[path = "../../services/tests/nary_n10_common/mod.rs"]
mod nary_n10_common;
mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::ai::registry::seam::Brain;
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_n10_common::owner;
use support::TestApp;
use uuid::Uuid;

const TAG: &str = "nary_fix4_files";

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

fn app() -> &'static TestApp {
    support::app(TAG)
}

async fn post_file(
    app: &TestApp,
    token: &str,
    name: &str,
    mime: &str,
    data: Vec<u8>,
) -> support::Resp {
    let part = reqwest::multipart::Part::bytes(data)
        .file_name(name.to_string())
        .mime_str(mime)
        .unwrap();
    app.send(
        app.client()
            .post(app.url("/api/v1/nary/chat-files"))
            .bearer_auth(token)
            .multipart(reqwest::multipart::Form::new().part("file", part)),
    )
    .await
}

#[tokio::test]
async fn test_the_web_chat_paperclip_takes_any_file() {
    let app = app();
    let b = Brain::new();
    let _seam = b.install().await; // one test at a time (the store is process-wide)
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    let mut db = app.db().await;
    let me = owner(&mut db).await;
    let token = app.token_for(me);

    let pdf = b"%PDF-1.4 Globex technical query".to_vec();
    let r = post_file(
        app,
        &token,
        "Globex query.pdf",
        "application/pdf",
        pdf.clone(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    let fid = out["file_id"].as_str().unwrap().to_string();
    assert_eq!(
        (out["type"].as_str(), out["name"].as_str()),
        (Some("pdf"), Some("Globex query.pdf"))
    );
    let line = out["chat_line"].as_str().unwrap();
    assert!(
        line.contains(&format!("file_id {fid}")) && line.contains("read_attachment"),
        "{line}"
    );
    let again = post_file(app, &token, "Globex query (1).pdf", "application/pdf", pdf).await;
    assert_eq!(again.json()["file_id"].as_str(), Some(fid.as_str()));
    let (user_id,): (Option<Uuid>,) =
        sqlx::query_as("SELECT chat_user_id FROM nary_file_links WHERE file_id = $1::uuid LIMIT 1")
            .bind(&fid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(user_id, Some(me));

    // the Owner's notes file: a meeting file, as before
    let r = post_file(
        app,
        &token,
        "Notes by Gemini.txt",
        "text/plain",
        b"Robert: 8 laptops".to_vec(),
    )
    .await;
    let out = r.json();
    assert_eq!(out["kind"], "notes", "{out}");
    assert!(out["chat_line"]
        .as_str()
        .unwrap()
        .contains("attach_meeting_notes"));

    // a staff member without nary:approve: the same kind of file is a plain file
    let staff = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Staff D1', 'x', true, false, false, 'internal')",
    )
    .bind(staff)
    .bind(format!("d1-{}@granary.test", &staff.simple().to_string()[..8]))
    .execute(&mut db)
    .await
    .unwrap();
    let r = post_file(
        app,
        &app.token_for(staff),
        "notes.txt",
        "text/plain",
        b"plain notes".to_vec(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    assert_eq!(out["type"], "text", "{out}");
    assert!(out["chat_line"]
        .as_str()
        .unwrap()
        .contains("read_attachment"));
    storage::set_store(None);
}
