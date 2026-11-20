//! Replays compat-tests/golden/google_calendar/session.json (first recorded from the retired Python
//! module; neutral account since): the same session against the same fake Google must send
//! byte-identical requests (method, path, raw query, content type, body) and return the same results.
//! Random values are masked on both sides: the OAuth state and the Meet requestId.

mod support;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, TimeDelta, TimeZone, Timelike, Utc};
use granary_core::errors::ApiError;
use granary_services::google_calendar as gc;
use serde_json::{json, Value};
use sqlx::PgConnection;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct State {
    token_calls: usize,
    events: usize,
    deleted: Vec<String>,
    attendees: std::collections::HashMap<String, Vec<String>>,
    token_error: Option<String>,
    requests: Vec<Value>,
}

struct FakeGoogle {
    state: Arc<Mutex<State>>,
    busy: Value,
}

fn mask(s: &str) -> String {
    let re = regex::Regex::new(r#""requestId":"[0-9a-f]{32}""#).unwrap();
    re.replace_all(s, r#""requestId":"{request_id}""#)
        .into_owned()
}

fn json_resp(status: u16, v: Value) -> ResponseTemplate {
    // httpx.Response(json=...) bodies: compact, UTF-8 kept.
    ResponseTemplate::new(status).set_body_raw(serde_json::to_vec(&v).unwrap(), "application/json")
}

impl Respond for FakeGoogle {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.state.lock().unwrap();
        let body = String::from_utf8(req.body.clone()).unwrap();
        let header = |n: &str| {
            req.headers
                .get(n)
                .map(|v| Value::String(v.to_str().unwrap().to_string()))
        };
        st.requests.push(json!({
            "method": req.method.to_string(), "path": req.url.path(), "query": req.url.query().unwrap_or(""),
            "authorization": header("authorization").unwrap_or(Value::Null),
            "content_type": header("content-type").unwrap_or(Value::Null), "body": mask(&body),
        }));
        let path = req.url.path().to_string();
        if path == "/token" {
            st.token_calls += 1;
            if let Some(e) = &st.token_error {
                return json_resp(400, json!({"error": e}));
            }
            return json_resp(
                200,
                json!({"access_token": format!("at-{}", st.token_calls), "expires_in": 3600,
                       "refresh_token": "rt-new", "scope": "calendar"}),
            );
        }
        if path == "/calendar/v3/freeBusy" {
            let b: Value = serde_json::from_str(&body).unwrap();
            let cal = b["items"][0]["id"].as_str().unwrap().to_string();
            let mut cals = serde_json::Map::new();
            cals.insert(cal, json!({"busy": self.busy}));
            return json_resp(200, json!({"calendars": cals}));
        }
        let eid = path.rsplit('/').next().unwrap().to_string();
        let method = req.method.to_string();
        if method == "DELETE" {
            if st.deleted.contains(&eid) {
                return json_resp(
                    410,
                    json!({"error": {"message": "Resource has been deleted"}}),
                );
            }
            st.deleted.push(eid);
            return ResponseTemplate::new(204);
        }
        if method == "GET" {
            if st.deleted.contains(&eid) {
                return json_resp(
                    410,
                    json!({"error": {"message": "Resource has been deleted"}}),
                );
            }
            let att: Vec<Value> = st
                .attendees
                .get(&eid)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|e| json!({"email": e, "responseStatus": "needsAction"}))
                .collect();
            return json_resp(
                200,
                json!({"id": eid, "status": "confirmed", "attendees": att}),
            );
        }
        let data: Value = serde_json::from_str(&body).unwrap();
        let emails = |d: &Value| -> Vec<String> {
            d.get("attendees")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .map(|x| x["email"].as_str().unwrap().to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        if method == "PATCH" {
            st.attendees.insert(eid.clone(), emails(&data));
            let mut out = serde_json::Map::new();
            out.insert("id".into(), json!(eid));
            for (k, v) in data.as_object().unwrap() {
                out.insert(k.clone(), v.clone());
            }
            return json_resp(200, Value::Object(out));
        }
        st.events += 1;
        let eid = format!("evt{}", st.events);
        st.attendees.insert(eid.clone(), emails(&data));
        if st.events % 2 == 1 {
            return json_resp(
                200,
                json!({"id": eid, "hangoutLink": format!("https://meet.google.com/abc-{eid}"),
                       "htmlLink": format!("https://calendar.google.com/{eid}")}),
            );
        }
        json_resp(
            200,
            json!({"id": eid, "hangoutLink": "", "conferenceData": {"entryPoints": [
                {"entryPointType": "phone", "uri": "tel:+1"},
                {"entryPointType": "video", "uri": format!("https://meet.google.com/v-{eid}")}]},
                "htmlLink": format!("https://calendar.google.com/{eid}")}),
        )
    }
}

/// `datetime.isoformat()` of an aware datetime.
fn py_iso(d: &DateTime<FixedOffset>) -> String {
    let mut s = d.format("%Y-%m-%dT%H:%M:%S").to_string();
    let us = d.nanosecond() / 1000;
    if us != 0 {
        s.push_str(&format!(".{us:06}"));
    }
    s + &d.format("%:z").to_string()
}

fn err_json(e: ApiError) -> Value {
    match e {
        ApiError::Http { status, detail, .. } => {
            json!({"status": status.as_u16(), "detail": detail})
        }
        other => panic!("unexpected error {other}"),
    }
}

async fn set(db: &mut PgConnection, key: &str, value: &str) {
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = $2")
        .bind(value)
        .bind(key)
        .execute(&mut *db)
        .await
        .unwrap();
}

async fn expire(db: &mut PgConnection) {
    sqlx::query("UPDATE google_accounts SET expires_at = now() - interval '1 minute'")
        .execute(&mut *db)
        .await
        .unwrap();
}

#[tokio::test]
async fn google_calendar_session_matches_python() {
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compat-tests/golden/google_calendar/session.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let state = Arc::new(Mutex::new(State::default()));
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(FakeGoogle {
            state: state.clone(),
            busy: golden["busy"].clone(),
        })
        .mount(&server)
        .await;
    std::env::set_var("GOOGLE_AUTH_URL", "http://google.fake/auth");
    std::env::set_var("GOOGLE_TOKEN_URL", format!("{}/token", server.uri()));
    std::env::set_var("GOOGLE_API_BASE", server.uri());
    std::env::set_var("API_URL", "https://erp.example.com/");

    let pool = support::fresh_db("s7_gcal_golden").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;

    sqlx::query("DELETE FROM google_accounts")
        .execute(&mut *db)
        .await
        .unwrap();
    set(
        db,
        "nary.google.client_id",
        golden["client_id"].as_str().unwrap(),
    )
    .await;
    set(
        db,
        "nary.google.client_secret",
        &granary_services::email_crypto::encrypt(golden["secret"].as_str().unwrap()),
    )
    .await;
    set(db, "nary.google.calendar_id", "primary").await;
    set(db, "nary.google.account_email", "calendar@example.com").await;
    let uid: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let me = granary_core::auth::load_active_user(db, uid)
        .await
        .unwrap()
        .unwrap();

    let mut steps: Vec<(String, Value)> = Vec::new();
    let record = |name: &str, result: Value, steps: &mut Vec<(String, Value)>| {
        let reqs = std::mem::take(&mut state.lock().unwrap().requests);
        steps.push((
            name.to_string(),
            json!({"step": name, "result": result, "requests": reqs}),
        ));
    };
    let now = Utc::now();
    let start: DateTime<Utc> = FixedOffset::east_opt(7 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 10, 20, 9, 0, 0)
        .unwrap()
        .with_nanosecond(123_456_000)
        .unwrap()
        .with_timezone(&Utc);
    let end: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 10, 27, 0, 0, 0).unwrap();
    let busy_json = |b: Vec<(DateTime<FixedOffset>, DateTime<FixedOffset>)>| -> Value {
        Value::Array(
            b.iter()
                .map(|(a, z)| json!([py_iso(a), py_iso(z)]))
                .collect(),
        )
    };

    record("status_before", gc::status(db).await.unwrap(), &mut steps);
    record(
        "free_busy_not_connected",
        err_json(gc::free_busy(db, now, now).await.unwrap_err()),
        &mut steps,
    );
    let url = gc::start_connect(db, &me).await.unwrap();
    let st = url
        .split("state=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    record(
        "start_connect",
        json!(url.replace(&st, "{state}")),
        &mut steps,
    );
    record(
        "finish_connect_stale",
        err_json(gc::finish_connect(db, "x", "nope").await.unwrap_err()),
        &mut steps,
    );
    let acct = gc::finish_connect(db, "4/0Ab_x~y*z q+r", &st)
        .await
        .unwrap();
    record(
        "finish_connect",
        json!({"status": acct.status, "scopes": acct.scopes, "oauth_state": acct.oauth_state}),
        &mut steps,
    );
    record(
        "finish_connect_again",
        err_json(gc::finish_connect(db, "again", &st).await.unwrap_err()),
        &mut steps,
    );
    record(
        "status_connected",
        gc::status(db).await.unwrap(),
        &mut steps,
    );
    record(
        "free_busy",
        busy_json(gc::free_busy(db, start, end).await.unwrap()),
        &mut steps,
    );
    expire(db).await;
    gc::free_busy(db, start, end).await.unwrap();
    record("free_busy_refreshed", Value::Null, &mut steps);

    let attendees: Vec<Option<String>> = [
        Some(" A@X.com"),
        Some("a@x.com"),
        None,
        Some(""),
        Some("  "),
        Some("b@y.com"),
        Some("B@Y.COM "),
    ]
    .iter()
    .map(|o| o.map(str::to_string))
    .collect();
    let ev1 = gc::create_event(
        db,
        "Meeting ✓ \"demo\" <Acme>",
        "Line 1\nLine 2 \\ é",
        start,
        start + TimeDelta::hours(1),
        &attendees,
    )
    .await
    .unwrap();
    record("create_event", ev1.clone(), &mut steps);
    let ev2 = gc::create_event(db, "Solo", "", end, end + TimeDelta::minutes(30), &[])
        .await
        .unwrap();
    record("create_event_alone", ev2, &mut steps);
    let eid = ev1["id"].as_str().unwrap().to_string();
    let add: Vec<String> = ["c@z.com", "A@x.com", " C@Z.com ", "", " d@w.com"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    record(
        "add_attendees",
        json!(gc::add_attendees(db, &eid, &add).await.unwrap()),
        &mut steps,
    );
    record(
        "get_event",
        gc::get_event(db, &eid)
            .await
            .unwrap()
            .unwrap_or(Value::Null),
        &mut steps,
    );
    gc::cancel_event(db, &eid).await.unwrap();
    record("cancel_event", Value::Null, &mut steps);
    record(
        "get_event_deleted",
        gc::get_event(db, &eid)
            .await
            .unwrap()
            .unwrap_or(Value::Null),
        &mut steps,
    );
    gc::cancel_event(db, &eid).await.unwrap();
    record("cancel_event_again", Value::Null, &mut steps);
    record(
        "add_attendees_deleted",
        err_json(
            gc::add_attendees(db, &eid, &["x@y.z".to_string()])
                .await
                .unwrap_err(),
        ),
        &mut steps,
    );
    set(
        db,
        "nary.google.calendar_id",
        "  team@group.calendar.google.com ",
    )
    .await;
    record(
        "free_busy_other_calendar",
        busy_json(gc::free_busy(db, start, end).await.unwrap()),
        &mut steps,
    );
    set(db, "nary.google.calendar_id", "primary").await;

    state.lock().unwrap().token_error = Some("server_error".into());
    expire(db).await;
    record(
        "refresh_failed",
        err_json(gc::free_busy(db, start, end).await.unwrap_err()),
        &mut steps,
    );

    sqlx::query("DELETE FROM google_accounts")
        .execute(&mut *db)
        .await
        .unwrap();
    set(db, "nary.google.client_id", "").await;
    set(db, "nary.google.client_secret", "").await;
    record(
        "start_connect_no_credentials",
        err_json(gc::start_connect(db, &me).await.unwrap_err()),
        &mut steps,
    );
    record(
        "status_no_credentials",
        gc::status(db).await.unwrap(),
        &mut steps,
    );

    let want = golden["steps"].as_array().unwrap();
    assert_eq!(want.len(), steps.len());
    for (w, (name, got)) in want.iter().zip(&steps) {
        // The Python fake runs at http://google.fake; the token and API calls here go to wiremock, whose
        // host is not part of what is compared (path, query, headers, body are).
        assert_eq!(w, got, "step {name}");
    }
}
