//! Nary batch 3 (agent R1): meeting recordings and notes files.
//!
//! - A recording uploaded for a meeting: stored privately, a job row; the minute tick sends it to the
//!   speech-to-text job API (wiremock), polls its progress, and when the transcript is in runs the batch 2
//!   debrief (results note on the deal, follow-ups, the stage/value card) and tells the Owner once.
//! - Failures (the job failed, refused at upload, longer than the limit, lost by a restart, past the deadline)
//!   are told to the Owner with the reason; size and type limits at upload.
//! - A notes file sent in the chat waits for its meeting; attach_meeting_notes finds the customer's latest past
//!   meeting (or lists meetings to pick from) and the tick reads it.
//! - The Owner sends a notes file to Nary on WhatsApp: it is kept and the assistant reads a line naming its file_id;
//!   someone without nary:approve gets the old behaviour.

mod nary_chat_common;
mod nary_n10_common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::{meeting_files, owner_channel, rhythm, staff_chat};
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_chat_common as c;
use nary_n10_common::{customer, deal_for, set, Cust};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TAG: &str = "nary_fix3_recordings";

// ─── Fakes ───────────────────────────────────────────────────────────────────

/// What the fake speech-to-text answers: POST /jobs and GET /jobs/{id}; the bodies it got.
struct Stt {
    submit: (u16, Value),
    status: (u16, Value),
    submitted: Vec<String>,
}

static STT: Lazy<Mutex<Stt>> = Lazy::new(|| {
    Mutex::new(Stt {
        submit: (202, json!({"job_id": "job-1", "status": "queued"})),
        status: (200, json!({"status": "queued", "progress": 0})),
        submitted: vec![],
    })
});

/// The fake sidecar, on its own thread and runtime (it outlives every test's runtime).
static STT_URL: Lazy<String> = Lazy::new(|| {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(|req: &Request| {
                    let mut st = STT.lock().unwrap();
                    let path = req.url.path().to_string();
                    if req.method.as_str() == "POST" && path == "/jobs" {
                        st.submitted
                            .push(String::from_utf8_lossy(&req.body).into_owned());
                        let (code, body) = st.submit.clone();
                        return ResponseTemplate::new(code).set_body_json(body);
                    }
                    if req.method.as_str() == "GET" && path.starts_with("/jobs/") {
                        let (code, body) = st.status.clone();
                        return ResponseTemplate::new(code).set_body_json(body);
                    }
                    ResponseTemplate::new(404)
                })
                .mount(&server)
                .await;
            tx.send(server.uri()).unwrap();
            std::future::pending::<()>().await;
        });
    });
    rx.recv().unwrap()
});

fn stt(submit: (u16, Value), status: (u16, Value)) {
    let mut st = STT.lock().unwrap();
    st.submit = submit;
    st.status = status;
}

fn submitted() -> Vec<String> {
    std::mem::take(&mut STT.lock().unwrap().submitted)
}

/// Before anything reads the settings (they are loaded once per process).
fn env() {
    std::env::set_var("STT_SERVICE_URL", STT_URL.as_str());
    std::env::set_var("STT_SERVICE_TOKEN", "stt-token");
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

fn store() -> Arc<MemStore> {
    let s = Arc::new(MemStore::default());
    storage::set_store(Some(s.clone()));
    s
}

fn extract() -> Value {
    json!({
        "summary": "Globex wants 12 laptops for two offices; budget approved by the office manager.",
        "bant": {"budget": 3, "authority": 2, "need": 3, "timeline": 2},
        "requirements": ["disk encryption"],
        "commitments": [{"who": "us", "what": "Send the solution proposal", "due": "2026-10-23"}],
        "objections": [], "next_steps": ["proposal review on Friday"], "language": "en",
        "deal_update": {"stage": "proposal", "value": 250000000, "currency": "JPY",
                        "reason": "They asked for the proposal and confirmed the budget."}})
}

fn brain() -> Brain {
    let b = Brain::p2();
    b.set("nary.transcript_extract", Answer::json(extract()));
    b.set(
        "nary.summary_email",
        Answer::json(json!({"subject": "Meeting summary", "body": "Thank you for your time..."})),
    );
    b
}

// ─── Builders ────────────────────────────────────────────────────────────────

/// A meeting with the customer `hours_ago` hours ago (negative: in the future).
async fn meeting(
    db: &mut PgConnection,
    cust: &Cust,
    deal_id: Option<Uuid>,
    title: &str,
    hours_ago: i64,
    language: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_meetings (id, lead_id, deal_id, contact_id, chat_id, channel, status, slots, origin, \
         attendees, title, starts_at, language) VALUES ($1, $2, $3, $4, $5, 'whatsapp', 'booked', '[]', 'chat', '[]', \
         $6, now() - make_interval(hours => $7), $8)",
    )
    .bind(id)
    .bind(cust.lead_id)
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(&cust.chat_id)
    .bind(title)
    .bind(hours_ago as i32)
    .bind(language)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn load_meeting(db: &mut PgConnection, id: Uuid) -> granary_db::rows::nary::NaryMeeting {
    sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn file(db: &mut PgConnection, id: Uuid) -> meeting_files::MeetingFile {
    meeting_files::load(db, id).await.unwrap().unwrap()
}

/// Make the file due for the next tick.
async fn due(db: &mut PgConnection, id: Uuid) {
    sqlx::query(
        "UPDATE nary_meeting_files SET next_poll_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
}

async fn tick(db: &mut PgConnection) -> Value {
    meeting_files::tick(db).await.unwrap()
}

/// The Owner-channel topics raised with this tag prefix: (tag, text).
async fn topics(db: &mut PgConnection, tag: &str) -> Vec<(String, String)> {
    sqlx::query_as("SELECT tag, text FROM nary_owner_topics WHERE tag LIKE $1 ORDER BY created_at")
        .bind(format!("{tag}%"))
        .fetch_all(&mut *db)
        .await
        .unwrap()
}

/// The world of one test: Nary's session pool, working hours, the Owner channel on.
async fn world(pool: &sqlx::PgPool) {
    granary_services::nary::set_session_pool(pool.clone());
    rhythm::set_working_time_override(Some(true));
    owner_channel::set_enabled_override(Some(true));
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn test_a_recording_is_transcribed_in_the_background_and_its_results_reach_the_deal() {
    env();
    c::run(TAG, |pool| async move {
        let b = brain();
        let _seam = b.install().await;
        world(&pool).await;
        let objects = store();
        let mut conn = pool.acquire().await.unwrap();
        let db: &mut PgConnection = &mut conn;
        let me = c::owner(db).await;
        let cust = customer(db, Some("budi@contoso.test"), true, "en").await;
        let deal_id = deal_for(db, &cust, "discovery").await;
        let mid = meeting(db, &cust, Some(deal_id), "Demo Globex", 2, Some("en")).await;
        let m = load_meeting(db, mid).await;

        let row = meeting_files::save_upload(
            db,
            &me,
            Some(&m),
            "Globex call.m4a",
            Some("audio/mp4"),
            b"....ftypM4A recording bytes",
            "upload",
        )
        .await
        .unwrap();
        assert_eq!(
            (row.kind.as_str(), row.status.as_str(), row.deal_id),
            ("recording", "queued", Some(deal_id))
        );
        assert!(objects.blobs.lock().unwrap().contains_key(&row.object_key));
        assert!(row.object_key.starts_with("nary/meetings/") && row.object_key.ends_with(".m4a"));

        // the tick sends it to speech-to-text, in the meeting's language, capped at the setting
        stt(
            (202, json!({"job_id": "job-rec-1", "status": "queued"})),
            (200, json!({"status": "queued", "progress": 0})),
        );
        submitted();
        let out = tick(db).await;
        assert_eq!(out[row.id.to_string()], "submitted", "{out}");
        let sent = submitted();
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].contains("name=\"language\"\r\n\r\nen"),
            "{}",
            sent[0]
        );
        assert!(sent[0].contains("name=\"max_seconds\"\r\n\r\n14400"));
        assert!(sent[0].contains("filename=\"Globex call.m4a\"") && sent[0].contains("ftypM4A"));
        let f = file(db, row.id).await;
        assert_eq!(
            (f.status.as_str(), f.stt_job_id.as_deref(), f.submits),
            ("transcribing", Some("job-rec-1"), 1)
        );
        // not due again before the poll interval
        assert_eq!(tick(db).await, json!({}));

        // progress
        due(db, row.id).await;
        stt(
            (202, json!({"job_id": "x"})),
            (
                200,
                json!({"job_id": "job-rec-1", "status": "transcribing", "progress": 40,
                         "text": null, "language": null, "duration": null, "error": null}),
            ),
        );
        assert_eq!(tick(db).await[row.id.to_string()], "transcribing");
        let f = file(db, row.id).await;
        assert_eq!((f.status.as_str(), f.progress), ("transcribing", 40));
        let shown = meeting_files::view(&f);
        assert_eq!(
            (shown["progress"].clone(), shown["has_transcript"].clone()),
            (json!(40), json!(false))
        );

        // done: the transcript is read like an uploaded one
        due(db, row.id).await;
        let transcript =
            "Robert: we need 12 laptops for two offices.\nCEO: we will send the proposal by Friday.";
        stt(
            (202, json!({"job_id": "x"})),
            (
                200,
                json!({"job_id": "job-rec-1", "status": "done", "progress": 100, "text": transcript,
                         "language": "en", "duration": 3600.0, "error": null}),
            ),
        );
        assert_eq!(tick(db).await[row.id.to_string()], "done");
        let f = file(db, row.id).await;
        assert_eq!((f.status.as_str(), f.progress), ("done", 100));
        assert_eq!(f.transcript.as_deref(), Some(transcript));
        assert_eq!(f.duration_seconds, Some(3600.0));
        let result = f.result.clone().unwrap();
        assert_eq!(result["commitments"], json!(1));
        let card_id: Uuid = result["deal_update_card"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let (action, payload): (String, Value) =
            sqlx::query_as("SELECT action, payload FROM agent_pending_actions WHERE id = $1")
                .bind(card_id)
                .fetch_one(&mut *db)
                .await
                .unwrap();
        assert_eq!(action, "crm.record_change");
        assert_eq!(payload["fields"]["stage"], "proposal");
        // the results note on the deal, from the recording
        let (subject, source): (String, Value) = sqlx::query_as(
            "SELECT subject, channel_data FROM activities WHERE deal_id = $1 AND activity_type = 'note'",
        )
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!(subject, "Meeting results: Demo Globex");
        assert_eq!(source["source"], "meeting recording (Globex call.m4a)");
        let reminders: Vec<String> = sqlx::query_scalar(
            "SELECT title FROM crm_reminders WHERE entity_type = 'deal' AND entity_id = $1",
        )
        .bind(deal_id)
        .fetch_all(&mut *db)
        .await
        .unwrap();
        assert!(
            reminders.contains(&"Send the solution proposal".to_string()),
            "{reminders:?}"
        );
        // one message for the Owner, with the stage/value change mentioned
        let told = topics(db, "Meeting results: Demo Globex").await;
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].1.contains("on the deal") && told[0].1.contains("waits for your approval"),
            "{}",
            told[0].1
        );
        assert!(told[0].1.contains("12 laptops"));
        // the transcript can be downloaded
        let (name, text) = meeting_files::transcript(db, row.id).await.unwrap();
        assert_eq!(
            (name.as_str(), text.as_str()),
            ("Globex call-transcript.txt", transcript)
        );
        // done is done: nothing more to do
        due(db, row.id).await;
        assert_eq!(tick(db).await, json!({}));
        storage::set_store(None);
    });
}

#[test]
fn test_failures_are_told_with_the_reason() {
    env();
    c::run(TAG, |pool| async move {
        let b = brain();
        let _seam = b.install().await;
        world(&pool).await;
        store();
        let mut conn = pool.acquire().await.unwrap();
        let db: &mut PgConnection = &mut conn;
        let me = c::owner(db).await;
        let cust = customer(db, None, true, "en").await;
        let mid = meeting(db, &cust, None, "Kickoff Acme", 3, None).await;
        let m = load_meeting(db, mid).await;
        let upload = |name: &'static str| {
            let me = me.clone();
            let m = m.clone();
            let pool = pool.clone();
            async move {
                let mut conn = pool.acquire().await.unwrap();
                meeting_files::save_upload(
                    &mut conn,
                    &me,
                    Some(&m),
                    name,
                    Some("audio/mpeg"),
                    b"ID3....",
                    "upload",
                )
                .await
                .unwrap()
            }
        };

        // the job fails on the service: the reason reaches the Owner
        submitted();
        let a = upload("a.mp3").await;
        stt(
            (202, json!({"job_id": "job-a"})),
            (
                200,
                json!({"status": "failed", "progress": 0, "error": "The file has no sound track"}),
            ),
        );
        tick(db).await;
        let sent = submitted();
        assert!(
            !sent[0].contains("name=\"language\""),
            "no language known: detected"
        );
        due(db, a.id).await;
        assert_eq!(tick(db).await[a.id.to_string()], "failed");
        let f = file(db, a.id).await;
        assert_eq!(
            (f.status.as_str(), f.error.as_deref()),
            ("failed", Some("The file has no sound track"))
        );
        let told = topics(db, "Couldn't read the recording: Kickoff Acme").await;
        assert_eq!(told.len(), 1);
        assert!(
            told[0].1.contains("a.mp3") && told[0].1.contains("The file has no sound track"),
            "{}",
            told[0].1
        );

        // refused at upload (too long): failed with the service's words, never polled
        let b2 = upload("b.mp3").await;
        stt(
            (
                413,
                json!({"detail": "The recording is 300 minutes; the limit is 240 minutes"}),
            ),
            (200, json!({"status": "queued"})),
        );
        assert_eq!(tick(db).await[b2.id.to_string()], "failed");
        assert_eq!(
            file(db, b2.id).await.error.as_deref(),
            Some("The recording is 300 minutes; the limit is 240 minutes")
        );

        // the service is down: it stays queued and is tried again later
        let d = upload("d.mp3").await;
        stt(
            (503, json!({"detail": "busy"})),
            (200, json!({"status": "queued"})),
        );
        assert_eq!(tick(db).await[d.id.to_string()], "waiting");
        let f = file(db, d.id).await;
        assert_eq!((f.status.as_str(), f.stt_job_id.clone()), ("queued", None));
        assert!(f.next_poll_at.unwrap() > chrono::Utc::now());

        // the transcript says it was longer than the limit (the length was unknown at upload)
        due(db, d.id).await;
        let old = set(db, &[("nary.meetings.recording_max_minutes", "60")]).await;
        stt(
            (202, json!({"job_id": "job-d"})),
            (
                200,
                json!({"status": "done", "progress": 100, "text": "long talk ...", "duration": 5400.0}),
            ),
        );
        tick(db).await;
        due(db, d.id).await;
        assert_eq!(tick(db).await[d.id.to_string()], "failed");
        assert_eq!(
            file(db, d.id).await.error.as_deref(),
            Some("the recording is longer than 60 minutes")
        );
        nary_n10_common::restore(db, old).await;

        // lost by a restart: sent again, then given up after nary.meetings.recording_resubmits
        let e = upload("e.mp3").await;
        let old = set(db, &[("nary.meetings.recording_resubmits", "1")]).await;
        stt(
            (202, json!({"job_id": "job-e"})),
            (404, json!({"detail": "No such job"})),
        );
        tick(db).await; // submitted (1)
        due(db, e.id).await;
        assert_eq!(tick(db).await[e.id.to_string()], "resubmit");
        let f = file(db, e.id).await;
        assert_eq!(
            (f.status.as_str(), f.stt_job_id.clone(), f.submits),
            ("queued", None, 1)
        );
        assert_eq!(tick(db).await[e.id.to_string()], "submitted"); // submitted (2)
        due(db, e.id).await;
        assert_eq!(tick(db).await[e.id.to_string()], "failed");
        assert!(file(db, e.id)
            .await
            .error
            .unwrap()
            .contains("too many times"));
        nary_n10_common::restore(db, old).await;

        // past the deadline
        let g = upload("g.mp3").await;
        sqlx::query(
            "UPDATE nary_meeting_files SET created_at = now() - interval '13 hours' WHERE id = $1",
        )
        .bind(g.id)
        .execute(&mut *db)
        .await
        .unwrap();
        assert_eq!(tick(db).await[g.id.to_string()], "failed");
        assert_eq!(
            file(db, g.id).await.error.as_deref(),
            Some("it was not transcribed within 12 hours")
        );

        // a recording with no speech is not debriefed
        let h = upload("h.mp3").await;
        stt(
            (202, json!({"job_id": "job-h"})),
            (
                200,
                json!({"status": "done", "progress": 100, "text": "", "duration": 30.0}),
            ),
        );
        tick(db).await;
        due(db, h.id).await;
        assert_eq!(tick(db).await[h.id.to_string()], "failed");
        assert_eq!(
            file(db, h.id).await.error.as_deref(),
            Some("no speech was heard in it")
        );
        assert!(b
            .calls()
            .iter()
            .all(|(job, _)| job != "nary.transcript_extract"));
        storage::set_store(None);
    });
}

#[test]
fn test_size_and_type_limits_at_upload() {
    env();
    c::run(TAG, |pool| async move {
        world(&pool).await;
        store();
        let mut conn = pool.acquire().await.unwrap();
        let db: &mut PgConnection = &mut conn;
        let me = c::owner(db).await;
        let old = set(
            db,
            &[
                ("nary.meetings.recording_max_mb", "1"),
                ("nary.meetings.notes_max_mb", "1"),
            ],
        )
        .await;
        let big = vec![b'x'; 1024 * 1024 + 1];
        let status = |e: granary_core::errors::ApiError| match e {
            granary_core::errors::ApiError::Http { status, detail, .. } => {
                (status.as_u16(), detail)
            }
            other => panic!("{other:?}"),
        };
        let e = meeting_files::save_upload(db, &me, None, "call.wav", None, &big, "web_chat")
            .await
            .unwrap_err();
        assert_eq!(
            status(e),
            (413, json!("The recording is too large (1 MB at most)"))
        );
        let e = meeting_files::save_upload(db, &me, None, "notes.docx", None, &big, "web_chat")
            .await
            .unwrap_err();
        assert_eq!(
            status(e),
            (413, json!("The notes file is too large (1 MB at most)"))
        );
        let e = meeting_files::save_upload(
            db,
            &me,
            None,
            "quote.pdf",
            Some("application/pdf"),
            b"%PDF",
            "web_chat",
        )
        .await
        .unwrap_err();
        assert_eq!(status(e).0, 415);
        let e = meeting_files::save_upload(db, &me, None, "x.txt", None, b"", "web_chat")
            .await
            .unwrap_err();
        assert_eq!(status(e).0, 400);
        // the kind comes from the name, else the mime type
        assert_eq!(
            meeting_files::kind_of("Meeting.MP4", None),
            Some("recording")
        );
        assert_eq!(
            meeting_files::kind_of("voice", Some("audio/ogg; codecs=opus")),
            Some("recording")
        );
        assert_eq!(
            meeting_files::kind_of("notes", Some("text/plain")),
            Some("notes")
        );
        assert_eq!(
            meeting_files::kind_of("photo.jpg", Some("image/jpeg")),
            None
        );
        nary_n10_common::restore(db, old).await;
        storage::set_store(None);
    });
}

#[test]
fn test_a_notes_file_from_the_chat_goes_to_the_customers_latest_past_meeting() {
    env();
    c::run(TAG, |pool| async move {
        let b = brain();
        let _seam = b.install().await;
        world(&pool).await;
        store();
        let mut conn = pool.acquire().await.unwrap();
        let db: &mut PgConnection = &mut conn;
        let me = c::owner(db).await;
        let cust = customer(db, Some("robert@contoso.test"), true, "en").await;
        let org: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
            .bind(cust.org_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
        let deal_id = deal_for(db, &cust, "discovery").await;
        let _older = meeting(db, &cust, Some(deal_id), "First call Contoso", 72, None).await;
        let latest = meeting(db, &cust, Some(deal_id), "Demo Contoso", 4, None).await;
        let _future = meeting(db, &cust, Some(deal_id), "Follow-up Contoso", -24, None).await;

        let notes =
            "Gemini notes\nRobert confirmed 12 laptops for two offices.\nNext: proposal by Friday.";
        let f = meeting_files::save_upload(
            db,
            &me,
            None,
            "Notes by Gemini.txt",
            Some("text/plain"),
            notes.as_bytes(),
            "web_chat",
        )
        .await
        .unwrap();
        assert_eq!(
            (f.kind.as_str(), f.status.as_str(), f.meeting_id),
            ("notes", "stored", None)
        );
        let line = meeting_files::chat_line(&f);
        assert!(
            line.contains(&f.id.to_string()) && line.contains("attach_meeting_notes"),
            "{line}"
        );
        // a stored file waits: the tick leaves it alone
        assert_eq!(tick(db).await, json!({}));

        // no meeting named: Nary gets the latest past meetings to ask about
        let out =
            meeting_files::_h_attach_meeting_notes(db, &me, json!({"file_id": f.id.to_string()}))
                .await
                .unwrap();
        assert!(
            out["error"].as_str().unwrap().contains("Say which meeting"),
            "{out}"
        );
        let listed: Vec<String> = out["meetings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["title"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(
            listed.contains(&"Demo Contoso".to_string())
                && !listed.contains(&"Follow-up Contoso".to_string()),
            "{listed:?}"
        );
        // an unknown customer: said so
        let out = meeting_files::_h_attach_meeting_notes(
            db,
            &me,
            json!({"file_id": f.id.to_string(), "customer": "Nobody Corp"}),
        )
        .await
        .unwrap();
        assert!(out["error"]
            .as_str()
            .unwrap()
            .contains("No past meeting with Nobody Corp"));
        // someone without nary:approve may not
        let staff = c::make_person(db, false, &["crm:view_own"]).await;
        let out = meeting_files::_h_attach_meeting_notes(
            db,
            &staff,
            json!({"file_id": f.id.to_string()}),
        )
        .await
        .unwrap();
        assert!(out["error"].as_str().unwrap().contains("nary:approve"));

        // the customer named: their latest past meeting
        // a partial name: the organisation name without its first word
        let short = org
            .split_once(' ')
            .map_or(org.as_str(), |(_, rest)| rest)
            .to_string();
        let out = meeting_files::_h_attach_meeting_notes(
            db,
            &me,
            json!({"file_id": f.id.to_string(), "customer": short}),
        )
        .await
        .unwrap();
        assert_eq!(out["done"], true, "{out}");
        assert_eq!(out["meeting"]["meeting_id"], latest.to_string());
        let row = file(db, f.id).await;
        assert_eq!(
            (row.status.as_str(), row.meeting_id, row.deal_id),
            ("queued", Some(latest), Some(deal_id))
        );
        // asked again while it is being read: no second run
        let again = meeting_files::_h_attach_meeting_notes(
            db,
            &me,
            json!({"file_id": f.id.to_string(), "meeting_id": latest.to_string()}),
        )
        .await
        .unwrap();
        assert!(again["note"].as_str().unwrap().contains("already reading"));

        // the tick reads it: the batch 2 debrief, with the notes as the record
        assert_eq!(tick(db).await[f.id.to_string()], "done");
        let row = file(db, f.id).await;
        assert_eq!(row.status, "done");
        assert_eq!(row.transcript.as_deref(), Some(notes));
        let calls = b.calls();
        let (_, call) = calls
            .iter()
            .find(|(job, _)| job == "nary.transcript_extract")
            .unwrap();
        assert!(format!("{call:?}").contains("Robert confirmed 12 laptops"));
        let source: Value = sqlx::query_scalar(
            "SELECT channel_data FROM activities WHERE deal_id = $1 AND activity_type = 'note'",
        )
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!(source["source"], "meeting notes (Notes by Gemini.txt)");
        assert_eq!(source["meeting_id"], latest.to_string());
        assert_eq!(topics(db, "Meeting results: Demo Contoso").await.len(), 1);
        storage::set_store(None);
    });
}

#[test]
fn test_the_owner_sends_a_notes_file_on_whatsapp_and_nary_gets_its_file_id() {
    env();
    c::run(TAG, |pool| async move {
        world(&pool).await;
        let objects = store();
        let mut conn = pool.acquire().await.unwrap();
        let db: &mut PgConnection = &mut conn;
        let old = c::set_settings(db, &[("nary.staff_chat.enabled", "true")]).await;
        let me = c::make_person(db, true, &[]).await;
        let cid = c::chat_id();
        c::link_staff_chat(db, me.id, &cid, "phone").await;
        granary_services::nary::media::seams::set_download(Some(Arc::new(|_id| {
            Box::pin(async {
                (
                    "stored".to_string(),
                    Some(b"Gemini notes: 12 laptops".to_vec()),
                    Some("text/plain".to_string()),
                )
            })
        })));

        let msg_id = c::ingest(
            db,
            &cid,
            "notes from today's Globex meeting",
            None,
            "document",
        )
        .await;
        sqlx::query("UPDATE wa_messages SET media_name = 'Notes by Gemini.txt', media_mime = 'text/plain', media_size = 24 WHERE id = $1")
            .bind(msg_id)
            .execute(&mut *db)
            .await
            .unwrap();
        let fake = c::FakeAssistant::new("Which meeting is it for?", vec![]);
        let out = staff_chat::seams::scope(
            c::seams_with(Some(fake.clone()), None),
            staff_chat::reply(db, &cid, &msg_id.to_string(), false),
        )
        .await
        .unwrap();
        assert_eq!(out["status"], "answered", "{out}");
        let key: String = sqlx::query_scalar("SELECT message_key FROM wa_messages WHERE id = $1")
            .bind(msg_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
        let (fid, kind, status, source, object_key): (Uuid, String, String, String, String) = sqlx::query_as(
            "SELECT id, kind, status, source, object_key FROM nary_meeting_files WHERE wa_message_key = $1",
        )
        .bind(&key)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!(
            (kind.as_str(), status.as_str(), source.as_str()),
            ("notes", "stored", "whatsapp")
        );
        assert_eq!(
            objects
                .blobs
                .lock()
                .unwrap()
                .get(&object_key)
                .map(|b| b.len()),
            Some(24)
        );
        let seen = fake.seen();
        let said = seen[0]["message"].as_str().unwrap();
        assert!(
            said.starts_with("notes from today's Globex meeting\n"),
            "{said}"
        );
        assert!(
            said.contains(&format!("file_id {fid}"))
                && said.contains("Meeting notes file attached"),
            "{said}"
        );

        // a person without nary:approve: not a meeting file; since batch 4 (D1) any file sent to Nary is kept for
        // its sender and named for read_attachment
        let (person, pcid) = c::staff(db).await;
        let _ = person;
        let pid = c::ingest(db, &pcid, "this is a doc", None, "document").await;
        sqlx::query(
            "UPDATE wa_messages SET media_name = 'x.txt', media_mime = 'text/plain' WHERE id = $1",
        )
        .bind(pid)
        .execute(&mut *db)
        .await
        .unwrap();
        let fake2 = c::FakeAssistant::new("OK", vec![]);
        staff_chat::seams::scope(
            c::seams_with(Some(fake2.clone()), None),
            staff_chat::reply(db, &pcid, &pid.to_string(), false),
        )
        .await
        .unwrap();
        let said2 = fake2.seen()[0]["message"].as_str().unwrap().to_string();
        assert!(
            said2.starts_with("this is a doc\n[File attached: x.txt")
                && said2.contains("read_attachment"),
            "{said2}"
        );
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM nary_meeting_files WHERE requested_by_id = $1",
        )
        .bind(person.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!(n, 0);
        granary_services::nary::media::seams::set_download(None);
        c::restore(db, old).await;
        storage::set_store(None);
    });
}

#[test]
fn test_the_tool_is_listed_with_a_valid_schema_and_not_for_read_only_connections() {
    use granary_services::ai::tools::catalog;
    use granary_services::ai::{mcp_access, schema};
    let t = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "attach_meeting_notes")
        .expect("attach_meeting_notes");
    let sent = schema::for_anthropic(&t.parameters);
    assert!(
        schema::violations(&sent).is_empty(),
        "{:?}",
        schema::violations(&sent)
    );
    assert_eq!(t.parameters["required"], json!(["file_id"]));
    assert!(mcp_access::PROPOSAL_TOOLS.contains(&"attach_meeting_notes"));
}
