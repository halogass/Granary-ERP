//! Nary batch 3 (agent R1) through the real app: meeting notes files and recordings.
//!
//! - A notes .txt uploaded on Nary → Meetings (the existing transcript upload) runs the whole batch 2 debrief:
//!   the results note and the follow-ups on the deal, the stage/value card.
//! - POST /nary/meetings/{id}/recording stores the recording and queues it (202, status and progress shown by
//!   GET /nary/meeting-files); the same permission as a transcript upload; size and type limits.
//! - POST /nary/meeting-files (the web chat's attach) stores a file with no meeting and gives the chat line.
//! - GET /nary/meeting-files/{id}/transcript downloads the transcript.

#[path = "../../services/tests/nary_n10_common/mod.rs"]
mod nary_n10_common;
mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::rhythm;
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_n10_common::{customer, deal_for, owner, Cust};
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{TestApp, UserSpec};
use uuid::Uuid;

const TAG: &str = "nary_fix3_recordings";

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

fn brain() -> Brain {
    let b = Brain::p2();
    b.set(
        "nary.transcript_extract",
        Answer::json(json!({
            "summary": "Globex wants 8 laptops for the new office; budget agreed.",
            "bant": {"budget": 3, "authority": 3, "need": 3, "timeline": 2},
            "requirements": ["disk encryption"],
            "commitments": [{"who": "us", "what": "Send the proposal", "due": "2026-10-24"}],
            "objections": [], "next_steps": ["proposal"], "language": "en",
            "deal_update": {"stage": "proposal", "value": 180000000, "currency": "JPY",
                            "reason": "Budget agreed, proposal requested."}})),
    );
    b.set(
        "nary.summary_email",
        Answer::json(json!({"subject": "Meeting summary", "body": "Thank you..."})),
    );
    b
}

async fn meeting(db: &mut PgConnection, cust: &Cust, deal_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_meetings (id, lead_id, deal_id, contact_id, chat_id, channel, status, slots, origin, \
         attendees, title, starts_at) VALUES ($1, $2, $3, $4, $5, 'whatsapp', 'booked', '[]', 'chat', '[]', $6, \
         now() - interval '2 hours')",
    )
    .bind(id)
    .bind(cust.lead_id)
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(&cust.chat_id)
    .bind(title)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn post_file(
    app: &TestApp,
    token: &str,
    path: &str,
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
            .post(app.url(path))
            .bearer_auth(token)
            .multipart(reqwest::multipart::Form::new().part("file", part)),
    )
    .await
}

#[tokio::test]
async fn test_a_notes_txt_upload_runs_the_whole_debrief_on_the_deal() {
    let app = app();
    let b = brain();
    let _seam = b.install().await;
    rhythm::set_working_time_override(Some(true));
    let mut db = app.db().await;
    let me = owner(&mut db).await;
    let token = app.token_for(me);
    let cust = customer(&mut db, Some("robert.globex@customer.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "discovery").await;
    let mid = meeting(&mut db, &cust, deal_id, "Demo Globex").await;
    let r = post_file(
        app,
        &token,
        &format!("/api/v1/nary/meetings/{mid}/debrief"),
        "Notes by Gemini.txt",
        "text/plain",
        b"Notes by Gemini\nRobert: 8 laptops for the new office. Budget agreed. Send the proposal."
            .to_vec(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    assert_eq!(out["commitments"], json!(1));
    assert!(out["deal_update_card"].is_string(), "{out}");
    assert!(out["summary_card"].is_string(), "{out}");
    let (subject, data): (String, Value) = sqlx::query_as(
        "SELECT subject, channel_data FROM activities WHERE deal_id = $1 AND activity_type = 'note'",
    )
    .bind(deal_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(subject, "Meeting results: Demo Globex");
    assert_eq!(data["source"], "meeting transcript (Notes by Gemini.txt)");
    let reminders: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM crm_reminders WHERE entity_type = 'deal' AND entity_id = $1",
    )
    .bind(deal_id)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(reminders, vec!["Send the proposal".to_string()]);
    let (action, payload): (String, Value) =
        sqlx::query_as("SELECT action, payload FROM agent_pending_actions WHERE id = $1")
            .bind(
                out["deal_update_card"]
                    .as_str()
                    .unwrap()
                    .parse::<Uuid>()
                    .unwrap(),
            )
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        (action.as_str(), payload["fields"]["stage"].clone()),
        ("crm.record_change", json!("proposal"))
    );
    let calls = b.calls();
    let (_, call) = calls
        .iter()
        .find(|(job, _)| job == "nary.transcript_extract")
        .unwrap();
    assert!(format!("{call:?}").contains("8 laptops for the new office"));
}

#[tokio::test]
async fn test_recording_uploads_listing_chat_files_and_the_transcript_download() {
    let app = app();
    let b = brain();
    let _seam = b.install().await; // one test at a time (the store is process-wide)
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    let mut db = app.db().await;
    let me = owner(&mut db).await;
    let token = app.token_for(me);
    let cust = customer(&mut db, None, true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "discovery").await;
    let mid = meeting(&mut db, &cust, deal_id, "Kickoff Acme").await;
    let path = format!("/api/v1/nary/meetings/{mid}/recording");

    let r = post_file(
        app,
        &token,
        &path,
        "kickoff.m4a",
        "audio/mp4",
        b"....ftypM4A".to_vec(),
    )
    .await;
    assert_eq!(r.status, 202, "{}", r.text());
    let up = r.json();
    assert_eq!(
        (
            up["kind"].clone(),
            up["status"].clone(),
            up["progress"].clone(),
            up["meeting_id"].clone()
        ),
        (
            json!("recording"),
            json!("queued"),
            json!(0),
            json!(mid.to_string())
        )
    );
    assert_eq!(up["deal_id"], deal_id.to_string());
    let key: String = sqlx::query_scalar("SELECT object_key FROM nary_meeting_files WHERE id = $1")
        .bind(up["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(
        objects.blobs.lock().unwrap().get(&key).cloned(),
        Some(b"....ftypM4A".to_vec())
    );

    let r = app
        .get(
            &token,
            &format!("/api/v1/nary/meeting-files?meeting_id={mid}"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let items = r.json()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["filename"], "kickoff.m4a");
    assert_eq!(items[0]["status"], "queued");

    // the same people who may upload a transcript
    let viewer = app.make_user(UserSpec::new().perms(&["nary:view"])).await;
    let r = post_file(
        app,
        &viewer.token,
        &path,
        "kickoff.m4a",
        "audio/mp4",
        b"x".to_vec(),
    )
    .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (403, json!("Permission required: nary:approve"))
    );
    // limits
    app.set_setting("nary.meetings.recording_max_mb", "1").await;
    let r = post_file(
        app,
        &token,
        &path,
        "long.mp3",
        "audio/mpeg",
        vec![b'x'; 1024 * 1024 + 1],
    )
    .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (413, json!("The recording is too large (1 MB at most)"))
    );
    app.set_setting("nary.meetings.recording_max_mb", "500")
        .await;
    let r = post_file(
        app,
        &token,
        &path,
        "deck.pdf",
        "application/pdf",
        b"%PDF".to_vec(),
    )
    .await;
    assert_eq!(r.status, 415);
    let r = post_file(
        app,
        &token,
        &format!("/api/v1/nary/meetings/{}/recording", Uuid::new_v4()),
        "a.mp3",
        "audio/mpeg",
        b"x".to_vec(),
    )
    .await;
    assert_eq!(r.status, 404);

    // a large recording comes in pieces (Cloudflare cuts a request at 100 MB): put back together at the end
    let upload_id = Uuid::new_v4().to_string();
    let mut last = Value::Null;
    for (i, piece) in [b"AAA".to_vec(), b"BBB".to_vec(), b"CC".to_vec()]
        .into_iter()
        .enumerate()
    {
        let part = reqwest::multipart::Part::bytes(piece)
            .file_name("long meeting.mp4")
            .mime_str("video/mp4")
            .unwrap();
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("upload_id", upload_id.clone())
            .text("index", i.to_string())
            .text("total", "3");
        let r = app
            .send(
                app.client()
                    .post(app.url(&path))
                    .bearer_auth(&token)
                    .multipart(form),
            )
            .await;
        assert_eq!(r.status, 202, "{}", r.text());
        last = r.json();
        if i < 2 {
            assert_eq!(
                last,
                json!({"status": "uploading", "received": i + 1, "total": 3})
            );
        }
    }
    assert_eq!(
        (last["filename"].clone(), last["size_bytes"].clone()),
        (json!("long meeting.mp4"), json!(8))
    );
    let key: String = sqlx::query_scalar("SELECT object_key FROM nary_meeting_files WHERE id = $1")
        .bind(last["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(
        objects.blobs.lock().unwrap().get(&key).cloned(),
        Some(b"AAABBBCC".to_vec())
    );
    // a missing piece: upload again
    let part = reqwest::multipart::Part::bytes(b"ZZ".to_vec()).file_name("x.mp4");
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("upload_id", Uuid::new_v4().to_string())
        .text("index", "1")
        .text("total", "2");
    let r = app
        .send(
            app.client()
                .post(app.url(&path))
                .bearer_auth(&token)
                .multipart(form),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (
            400,
            json!("A piece of the upload is missing; upload the file again")
        )
    );

    // a file attached in the web chat: no meeting yet, and the line the chat sends with it
    let r = post_file(
        app,
        &token,
        "/api/v1/nary/meeting-files",
        "Notes by Gemini.docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        b"PK..".to_vec(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let chat = r.json();
    assert_eq!(
        (
            chat["kind"].clone(),
            chat["status"].clone(),
            chat["meeting_id"].clone()
        ),
        (json!("notes"), json!("stored"), Value::Null)
    );
    let line = chat["chat_line"].as_str().unwrap();
    assert!(
        line.contains(chat["id"].as_str().unwrap()) && line.contains("attach_meeting_notes"),
        "{line}"
    );

    // the transcript: none yet, then downloadable
    let fid = up["id"].as_str().unwrap();
    let tpath = format!("/api/v1/nary/meeting-files/{fid}/transcript");
    assert_eq!(app.get(&token, &tpath).await.status, 404);
    sqlx::query(
        "UPDATE nary_meeting_files SET transcript = 'Robert: hello everyone.' WHERE id = $1",
    )
    .bind(fid.parse::<Uuid>().unwrap())
    .execute(&mut db)
    .await
    .unwrap();
    let r = app.get(&token, &tpath).await;
    assert_eq!(
        (r.status, r.text()),
        (200, "Robert: hello everyone.".to_string())
    );
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        r.header("content-disposition").as_deref(),
        Some("attachment; filename=\"kickoff-transcript.txt\"")
    );
    let outsider = app
        .make_user(UserSpec::new().perms(&["crm:view_own"]))
        .await;
    assert_eq!(app.get(&outsider.token, &tpath).await.status, 403);
    storage::set_store(None);
}
