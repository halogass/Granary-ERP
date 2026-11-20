//! Port of the Google tests in backend/tests/test_nary_p3_meetings.py, with the `google` fixture of
//! tests/nary_p3_support.py: a fake Google (token, free/busy) and a connected account.
//!
//! The fake runs on its own runtime thread for the whole binary (settings are loaded once per process, so
//! every test must talk to the same server); tests run one at a time under a lock.

mod support;

use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, Utc};
use granary_core::auth::{load_active_user, AuthUser};
use granary_services::email_crypto::encrypt;
use granary_services::google_calendar as gc;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct State {
    token_calls: Vec<Vec<(String, String)>>,
    token_error: Option<String>,
    freebusy_calls: Vec<Value>,
}

struct Fake(Arc<Mutex<State>>);

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let path = req.url.path();
        if path == "/token" {
            let form: Vec<(String, String)> = url_decode(&String::from_utf8_lossy(&req.body));
            st.token_calls.push(form);
            if let Some(e) = &st.token_error {
                return ResponseTemplate::new(400).set_body_json(json!({"error": e}));
            }
            let n = st.token_calls.len();
            return ResponseTemplate::new(200).set_body_json(json!({
                "access_token": format!("at-{n}"), "expires_in": 3600, "refresh_token": "rt-new", "scope": "calendar"}));
        }
        if path == "/calendar/v3/freeBusy" {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            st.freebusy_calls.push(body.clone());
            let cal = body["items"][0]["id"].as_str().unwrap().to_string();
            let mut cals = serde_json::Map::new();
            cals.insert(cal, json!({"busy": []}));
            return ResponseTemplate::new(200).set_body_json(json!({"calendars": cals}));
        }
        ResponseTemplate::new(404)
    }
}

/// `dict(httpx.QueryParams(body))` for the simple bodies here.
fn url_decode(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (dec(k), dec(v))
        })
        .collect()
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

struct Google {
    state: Arc<Mutex<State>>,
    db_url: String,
}

static GOOGLE: Lazy<Google> = Lazy::new(|| {
    let state = Arc::new(Mutex::new(State::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    let st = state.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(Fake(st))
                .mount(&server)
                .await;
            let pool = support::fresh_db("s7_gcal_flow").await;
            // the baseline ships no calendar account: the company books meetings in this one
            sqlx::query(
                "UPDATE system_settings SET value = 'meetings@example.com' WHERE key = 'nary.google.account_email'",
            )
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
            tx.send((server.uri(), std::env::var("DATABASE_URL").unwrap()))
                .unwrap();
            std::future::pending::<()>().await;
        });
    });
    let (uri, db_url) = rx.recv().unwrap();
    std::env::set_var("GOOGLE_TOKEN_URL", format!("{uri}/token"));
    std::env::set_var("GOOGLE_API_BASE", &uri);
    Google { state, db_url }
});

static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn set(db: &mut PgConnection, key: &str, value: &str) {
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = $2")
        .bind(value)
        .bind(key)
        .execute(&mut *db)
        .await
        .unwrap();
}

/// The `google` fixture: credentials set, one connected account, a fresh fake.
async fn google() -> PgConnection {
    let g = &*GOOGLE;
    *g.state.lock().unwrap() = State::default();
    let mut db = PgConnection::connect(&g.db_url).await.unwrap();
    set(
        &mut db,
        "nary.google.client_id",
        "cid.apps.googleusercontent.com",
    )
    .await;
    set(&mut db, "nary.google.client_secret", &encrypt("csecret")).await;
    sqlx::query("DELETE FROM google_accounts")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO google_accounts (id, email, status, access_token, refresh_token, expires_at) \
         VALUES (gen_random_uuid(), 'meetings@example.com', 'connected', $1, $2, now() + interval '1 hour')",
    )
    .bind(encrypt("at-0"))
    .bind(encrypt("rt-0"))
    .execute(&mut db)
    .await
    .unwrap();
    db
}

async fn owner(db: &mut PgConnection) -> AuthUser {
    let id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    load_active_user(db, id).await.unwrap().unwrap()
}

fn grant_type(st: &State) -> String {
    st.token_calls
        .last()
        .unwrap()
        .iter()
        .find(|(k, _)| k == "grant_type")
        .unwrap()
        .1
        .clone()
}

#[tokio::test]
async fn test_connecting_google_asks_only_for_free_busy_and_events() {
    let _l = LOCK.lock().await;
    let mut db = google().await;
    let me = owner(&mut db).await;
    sqlx::query("DELETE FROM google_accounts")
        .execute(&mut db)
        .await
        .unwrap();
    let url = gc::start_connect(&mut db, &me).await.unwrap();
    assert!(url.contains("access_type=offline") && url.contains("prompt=consent"));
    assert!(
        url.contains("calendar.freebusy")
            && url.contains("calendar.events")
            && !url.contains("calendar.readonly")
    );
    let state = url
        .split("state=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    let acct = gc::finish_connect(&mut db, "the-code", &state)
        .await
        .unwrap();
    assert_eq!(acct.status, "connected");
    assert_ne!(acct.refresh_token.as_deref(), Some("rt-new")); // stored encrypted
    assert_eq!(
        grant_type(&GOOGLE.state.lock().unwrap()),
        "authorization_code"
    );
    assert!(gc::finish_connect(&mut db, "again", &state).await.is_err()); // one-time state
}

#[tokio::test]
async fn test_an_expired_token_is_refreshed() {
    let _l = LOCK.lock().await;
    let mut db = google().await;
    owner(&mut db).await;
    sqlx::query("UPDATE google_accounts SET expires_at = now() - interval '1 minute'")
        .execute(&mut db)
        .await
        .unwrap();
    let now = Utc::now();
    gc::free_busy(&mut db, now, now + TimeDelta::days(1))
        .await
        .unwrap();
    assert_eq!(grant_type(&GOOGLE.state.lock().unwrap()), "refresh_token");
    assert_eq!(GOOGLE.state.lock().unwrap().freebusy_calls.len(), 1);
}

/// The second half of test_an_expired_token_is_refreshed_and_a_revoked_one_asks_the_owner: a revoked
/// refresh token marks the account needs_reconnect and raises the Owner's error card (nary.governance).
#[tokio::test]
async fn test_a_revoked_token_asks_the_owner() {
    let _l = LOCK.lock().await;
    let mut db = google().await;
    GOOGLE.state.lock().unwrap().token_error = Some("invalid_grant".into());
    sqlx::query(
        "UPDATE google_accounts SET expires_at = now() - interval '1 minute', access_token = $1",
    )
    .bind(encrypt("x"))
    .execute(&mut db)
    .await
    .unwrap();
    let now = Utc::now();
    let e = gc::free_busy(&mut db, now, now + TimeDelta::days(1))
        .await
        .unwrap_err();
    assert!(gc::is_google_not_connected(&e));
    let status: String = sqlx::query_scalar("SELECT status FROM google_accounts")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(status, "needs_reconnect");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE title = 'Connect Google Calendar again'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(n > 0);
}

/// Inside the caller's transaction, `start_connect` commits like Python's `db.commit()` and leaves the
/// caller in a fresh, usable transaction.
#[tokio::test]
async fn start_connect_commits_inside_a_transaction() {
    let _l = LOCK.lock().await;
    let mut db = google().await;
    let me = owner(&mut db).await;
    let mut tx = db.begin().await.unwrap();
    let url = gc::start_connect(&mut tx, &me).await.unwrap();
    let state = url
        .split("state=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    drop(tx); // a rollback of what follows the commit: the state is already saved
    let saved: Option<String> = sqlx::query_scalar("SELECT oauth_state FROM google_accounts")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(saved.as_deref(), Some(state.as_str()));
}

#[tokio::test]
async fn not_connected_and_needs_reconnect_are_409() {
    let _l = LOCK.lock().await;
    let mut db = google().await;
    let now = Utc::now();
    sqlx::query("UPDATE google_accounts SET status = 'needs_reconnect'")
        .execute(&mut db)
        .await
        .unwrap();
    let e = gc::free_busy(&mut db, now, now).await.unwrap_err();
    assert!(gc::is_google_not_connected(&e));
    assert_eq!(
        e.body(),
        json!({"detail": "Google Calendar needs to be connected again (Nary → Meetings)."})
    );
    sqlx::query("UPDATE google_accounts SET status = 'pending'")
        .execute(&mut db)
        .await
        .unwrap();
    let e = gc::cancel_event(&mut db, "evt").await.unwrap_err();
    assert_eq!(e.status().as_u16(), 409);
    assert_eq!(
        e.body(),
        json!({"detail": "Google Calendar is not connected (Nary → Meetings → Connect Google)."})
    );
}
