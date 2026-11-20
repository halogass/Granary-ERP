//! routers/calls.py through the real app. Ports of the backend tests that set row state directly or call
//! Python internals:
//! - test_audit_fixes.py: test_signal_ticket_round_trip, test_signal_ticket_rejects_tampering_and_expiry
//!   (`_signal_ticket` / `_verify_signal_ticket`);
//! - test_audit_high_b2.py: test_join_leave_and_kick_are_saved (the room and its guest are inserted directly);
//! - test_audit_low_w2.py: test_call_recording_is_for_host_and_participants_only;
//!
//! Plus the recording round trip with an in-memory MinIO (Python's `_get_minio` monkeypatched).
//! The HTTP suite (compat-tests/http-suite/tests/test_calls_routes.py) pins every route on both stacks.

mod support;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use granary_api::routers::calls::{_signal_ticket, _verify_signal_ticket};
use granary_compat::pyjson::PyDateTime;
use granary_services::storage::{ObjectStore, StoreError};
use serde_json::{json, Value};
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "calls_routes";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// MinIO as a dict (bucket/key -> bytes) and the buckets made so far (none exists at first).
#[derive(Default)]
struct MemStore(
    Mutex<HashMap<String, (Vec<u8>, String)>>,
    Mutex<HashSet<String>>,
);

impl ObjectStore for MemStore {
    fn bucket_exists<'a, 'b, 'c>(&'a self, bucket: &'b str) -> Fut<'c, Result<bool, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async move { Ok(self.1.lock().unwrap().contains(bucket)) })
    }
    fn make_bucket<'a, 'b, 'c>(&'a self, bucket: &'b str) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.1.lock().unwrap().insert(bucket.to_string());
            Ok(())
        })
    }
    fn put_object<'a, 'b, 'd, 'e, 'f, 'c>(
        &'a self,
        bucket: &'b str,
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
            self.0.lock().unwrap().insert(
                format!("{bucket}/{key}"),
                (data.to_vec(), content_type.to_string()),
            );
            Ok(())
        })
    }
    fn get_object<'a, 'b, 'd, 'c>(
        &'a self,
        bucket: &'b str,
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
                .get(&format!("{bucket}/{key}"))
                .map(|(d, _)| d.clone())
                .ok_or(StoreError {
                    kind: "S3Error".into(),
                    message: "NoSuchKey".into(),
                })
        })
    }
}

fn now_iso() -> String {
    PyDateTime::from_chrono_utc(&Utc::now()).isoformat()
}

async fn add_room(
    host: Uuid,
    participants: Value,
    is_active: Option<bool>,
    max_participants: Option<i32>,
) -> String {
    let app = app(TAG);
    let mut db = app.db().await;
    let room_id = format!("pyt{}", &Uuid::new_v4().simple().to_string()[..12]);
    sqlx::query(
        "INSERT INTO video_rooms (id, room_id, name, created_by_id, is_active, max_participants, participants, \
         waiting_participants, created_at, updated_at) VALUES ($1, $2, 'B2', $3, $4, $5, $6, '[]', now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(&room_id)
    .bind(host)
    .bind(is_active)
    .bind(max_participants)
    .bind(participants)
    .execute(&mut db)
    .await
    .unwrap();
    room_id
}

async fn participants(room_id: &str) -> Vec<Value> {
    let mut db = app(TAG).db().await;
    let v: Option<Value> =
        sqlx::query_scalar("SELECT participants FROM video_rooms WHERE room_id = $1")
            .bind(room_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    v.and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

// ─── test_audit_fixes.py: the signal ticket (pure) ───────────────────────────

#[tokio::test]
async fn test_signal_ticket_round_trip() {
    let _ = app(TAG); // settings (APP_SECRET_KEY) as the server has them
    let ticket = _signal_ticket("room123", "erp_abcd1234", true, 6 * 3600);
    let claims = _verify_signal_ticket(&ticket).unwrap().expect("valid");
    assert_eq!(claims["room"], "room123");
    assert_eq!(claims["peer"], "erp_abcd1234");
    assert_eq!(claims["host"], true);
}

#[tokio::test]
async fn test_signal_ticket_rejects_tampering_and_expiry() {
    let _ = app(TAG);
    let ticket = _signal_ticket("room123", "erp_abcd1234", false, 6 * 3600);
    let (payload, sig) = ticket.split_once('.').unwrap();
    assert!(
        _verify_signal_ticket(&format!("{payload}.{}", "A".repeat(sig.len())))
            .unwrap()
            .is_none()
    );
    assert!(_verify_signal_ticket(payload).unwrap().is_none()); // no signature at all
    assert!(_verify_signal_ticket("not-a-ticket").unwrap().is_none());
    let expired = _signal_ticket("room123", "erp_abcd1234", false, -10);
    assert!(_verify_signal_ticket(&expired).unwrap().is_none());
}

// ─── test_audit_high_b2.py: H24 call participants ────────────────────────────

#[tokio::test]
async fn test_join_leave_and_kick_are_saved() {
    let app = app(TAG);
    let host = app.user_with(&[]).await;
    let alice = app.user_with(&[]).await;
    let bob = app.user_with(&[]).await;
    let wid = Uuid::new_v4().simple().to_string();
    let room = add_room(
        host.id,
        json!([{"user_id": format!("guest_{wid}"), "name": "Guest", "is_guest": true,
                "joined_at": now_iso(), "left_at": null}]),
        Some(true),
        Some(10),
    )
    .await;
    let base = format!("/api/v1/calls/rooms/{room}");
    assert_eq!(
        app.post(&alice.token, &format!("{base}/join"), json!({}))
            .await
            .status,
        200
    );
    // a re-join adds no second entry
    assert_eq!(
        app.post(&alice.token, &format!("{base}/join"), json!({}))
            .await
            .status,
        200
    );
    assert_eq!(
        app.post(&bob.token, &format!("{base}/join"), json!({}))
            .await
            .status,
        200
    );
    let parts = participants(&room).await;
    let alice_id = alice.id.to_string();
    assert_eq!(
        parts
            .iter()
            .filter(|p| p["user_id"] == alice_id.as_str())
            .count(),
        1
    );

    assert_eq!(
        app.post(&alice.token, &format!("{base}/leave"), json!({}))
            .await
            .status,
        200
    );
    let bob_peer = format!("erp_{}", &bob.id.to_string()[..8]);
    assert_eq!(
        app.post(&host.token, &format!("{base}/kick/{bob_peer}"), json!({}))
            .await
            .status,
        200
    );
    assert_eq!(
        app.post(
            &host.token,
            &format!("{base}/kick/guest_{}", &wid[..8]),
            json!({})
        )
        .await
        .status,
        200
    );
    let by_user: HashMap<String, Value> = participants(&room)
        .await
        .into_iter()
        .map(|p| (p["user_id"].as_str().unwrap().to_string(), p))
        .collect();
    let a = &by_user[&alice_id];
    assert!(a["left_at"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(a
        .get("kicked")
        .is_none_or(|k| k == &json!(null) || k == &json!(false)));
    let b = &by_user[&bob.id.to_string()];
    assert!(b["left_at"].as_str().is_some() && b["kicked"] == json!(true));
    assert_eq!(by_user[&format!("guest_{wid}")]["kicked"], json!(true));
}

#[tokio::test]
async fn a_room_without_a_max_participants_value_cannot_be_joined() {
    // `len(active) >= None` raises TypeError in Python: the unhandled 500.
    let app = app(TAG);
    let host = app.user_with(&[]).await;
    let room = add_room(host.id, json!([]), Some(true), None).await;
    let r = app
        .post(
            &host.token,
            &format!("/api/v1/calls/rooms/{room}/join"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 500);
}

// ─── test_audit_low_w2.py: L74 call recordings ───────────────────────────────

#[tokio::test]
async fn test_call_recording_is_for_host_and_participants_only() {
    let app = app(TAG);
    let host = app.user_with(&[]).await;
    let guest = app.user_with(&[]).await;
    let outsider = app.user_with(&[]).await;
    let room = add_room(
        host.id,
        json!([{"user_id": guest.id.to_string(), "name": "g"}]),
        None,
        None,
    )
    .await;
    let path = format!("/api/v1/calls/rooms/{room}/recording");
    assert_eq!(app.get(&outsider.token, &path).await.status, 403);
    for u in [&host, &guest] {
        // allowed past the check; there is no recording yet
        assert_eq!(app.get(&u.token, &path).await.status, 404);
    }
}

#[tokio::test]
async fn the_host_uploads_a_recording_and_participants_get_a_link() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let store = Arc::new(MemStore::default());
    granary_services::storage::set_store(Some(store.clone()));
    let host = app.user_with(&[]).await;
    let guest = app.user_with(&[]).await;
    let room = add_room(
        host.id,
        json!([{"user_id": guest.id.to_string(), "name": "g"}]),
        Some(true),
        Some(10),
    )
    .await;
    let path = format!("/api/v1/calls/rooms/{room}/recording");
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"webm-bytes".to_vec())
            .file_name("call.webm")
            .mime_str("video/webm")
            .unwrap(),
    );
    let r = app
        .send(
            app.client()
                .post(app.url(&path))
                .bearer_auth(&host.token)
                .multipart(form),
        )
        .await;
    granary_services::storage::set_store(None);
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let key = body["key"].as_str().unwrap().to_string();
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["status", "key", "size_bytes"]
    );
    assert_eq!(body["status"], "uploaded");
    assert_eq!(body["size_bytes"], 10);
    assert!(key.starts_with(&format!("recordings/{room}/")) && key.ends_with(".webm"));
    assert_eq!(
        key.len(),
        format!("recordings/{room}/20261016_093000.webm").len()
    );
    // the recordings bucket did not exist: the upload made it
    assert!(store.1.lock().unwrap().contains("granary-recordings"));
    let stored = store.0.lock().unwrap().clone();
    assert_eq!(
        stored.get(&format!("granary-recordings/{key}")),
        Some(&(b"webm-bytes".to_vec(), "video/webm".to_string()))
    );

    granary_services::storage::set_store(Some(store.clone()));
    let r = app.get(&guest.token, &path).await;
    granary_services::storage::set_store(None);
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let url = body["download_url"].as_str().unwrap();
    assert!(url.contains(&format!("/granary-recordings/{}", key)));
    assert!(url.contains("X-Amz-Expires=7200"));
    assert_eq!(body["size_bytes"], 10);
    assert!(body["recorded_at"].as_str().unwrap().ends_with("+00:00"));
    let mut db = app.db().await;
    let (rk, size): (Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT recording_key, recording_size_bytes FROM video_rooms WHERE room_id = $1",
    )
    .bind(&room)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((rk.as_deref(), size), (Some(key.as_str()), Some(10)));
}

#[tokio::test]
async fn invites_go_to_active_users_once() {
    let app = app(TAG);
    let host = app
        .make_user(support::UserSpec::new().name("Hana Host"))
        .await;
    let a = app.user_with(&[]).await;
    let r = app
        .post(
            &host.token,
            "/api/v1/calls/rooms",
            json!({"name": u("Ops"), "invited_user_ids": [a.id.to_string(), a.id.to_string()]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let room_id = r.json()["room_id"].as_str().unwrap().to_string();
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE entity_id = $1 AND user_id = $2 AND type = 'call_invite'",
    )
    .bind(&room_id)
    .bind(a.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 1);
    // A str that is not a UUID in a UUID column: asyncpg raises, the central 422.
    let r = app
        .post(
            &host.token,
            "/api/v1/calls/rooms",
            json!({"invited_user_ids": ["not-a-uuid"]}),
        )
        .await;
    assert_eq!(r.status, 422);
}
