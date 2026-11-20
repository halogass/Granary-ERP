//! Port of the HTTP tests of backend/tests/test_face_attendance.py (the five services-crate placeholders in
//! crates/services/tests/face_attendance.rs marked "router: routers/face_attendance.py ...", plus
//! `test_rate_limit_answers_429` through the route). The engine sidecar is a wiremock server
//! (FACE_SERVICE_URL): tests queue what `/analyze` returns, as the Python `Engine` fake does, and it serves
//! two asset files.
//!
//! Python disables the rate limiter with a monkeypatch; here every test runs serially and clears the
//! `face:rl:*` counters before each call. Synthetic identities come from a seeded Gaussian generator (numpy's
//! `default_rng(seed).normal(0, 0.088, 128)` in face_fixtures.py; only the statistics matter).
//! `test_weak_personal_match_goes_to_hr_review` is flaky in Python (the drift is random, ~0.55); here the
//! drift vector is scaled to a fixed length so the distance lands inside the 0.50-0.60 review band.

mod support;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

const HR_PERMS: &[&str] = &[
    "hr:manage_employees",
    "hr:manage_attendance",
    "hr:view_attendance",
];
const OFFICE: (f64, f64) = (-45.0311, 168.6626);

static QUEUE: Mutex<VecDeque<Value>> = Mutex::new(VecDeque::new());
/// What `/passive` answers next (migration 0166 tests).
static PASSIVE_QUEUE: Mutex<VecDeque<Value>> = Mutex::new(VecDeque::new());
/// The flash field names of the last `/passive` request.
static PASSIVE_FIELDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CALLS: AtomicUsize = AtomicUsize::new(0);

struct Engine;

impl Respond for Engine {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path();
        if path == "/passive" {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            let mut names: Vec<String> = body
                .split("form-data; name=\"")
                .skip(1)
                .filter_map(|p| p.split('"').next().map(str::to_string))
                .collect();
            names.sort();
            *PASSIVE_FIELDS.lock().unwrap() = names;
            let v = PASSIVE_QUEUE.lock().unwrap().pop_front();
            return match v {
                Some(v) => ResponseTemplate::new(200).set_body_json(v),
                None => ResponseTemplate::new(500),
            };
        }
        if path == "/analyze" {
            CALLS.fetch_add(1, Ordering::SeqCst);
            let images = QUEUE.lock().unwrap().pop_front().unwrap_or(json!([]));
            return ResponseTemplate::new(200).set_body_json(json!({ "images": images }));
        }
        match path {
            "/assets/face-api.esm.js" => ResponseTemplate::new(200)
                .insert_header("content-type", "application/octet-stream")
                .set_body_bytes(b"export const faceapi = {};".to_vec()),
            "/assets/face_recognition_model-weights_manifest.json" => ResponseTemplate::new(200)
                .insert_header("content-type", "application/octet-stream")
                .set_body_bytes(b"[]".to_vec()),
            _ => ResponseTemplate::new(404),
        }
    }
}

fn app() -> &'static support::TestApp {
    static URI: OnceLock<String> = OnceLock::new();
    URI.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let server = wiremock::MockServer::start().await;
                wiremock::Mock::given(wiremock::matchers::any())
                    .respond_with(Engine)
                    .mount(&server)
                    .await;
                tx.send(server.uri()).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let uri = rx.recv().unwrap();
        std::env::set_var("FACE_SERVICE_URL", &uri);
        uri
    });
    support::app("face_attendance")
}

async fn clear_limits(app: &support::TestApp) {
    let c = redis::Client::open(app.redis_url.as_str()).unwrap();
    let mut r = c.get_multiplexed_async_connection().await.unwrap();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("face:rl:*")
        .query_async(&mut r)
        .await
        .unwrap();
    if !keys.is_empty() {
        let _: () = redis::cmd("DEL")
            .arg(keys)
            .query_async(&mut r)
            .await
            .unwrap();
    }
}

// ─── face_fixtures.py ─────────────────────────────────────────────────────────

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self, sd: f64) -> f64 {
        let (u1, u2) = (self.next(), self.next());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos() * sd
    }
}

fn rng(seed: u64) -> Rng {
    Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
}

/// A stable identity descriptor (norm ~1, like face-api.js).
fn person(seed: u64) -> Vec<f64> {
    let mut r = rng(seed);
    (0..128).map(|_| r.normal(0.088)).collect()
}

/// Another shot of the same person (distance ~0.1).
fn same(base: &[f64], seed: u64) -> Vec<f64> {
    let mut r = rng(seed + 1000);
    base.iter()
        .map(|x| ((x + r.normal(0.01)) * 1e6).round() / 1e6)
        .collect()
}

fn landmarks(eye_open: f64, yaw_shift: f64) -> Value {
    let mut pts = vec![[0.0f64, 0.0]; 68];
    for (i, p) in pts.iter_mut().enumerate().take(17) {
        *p = [
            100.0 + i as f64 * 12.5,
            200.0 + (i as f64 - 8.0).abs() * -5.0,
        ];
    }
    for (i, p) in pts.iter_mut().enumerate().take(36).skip(27) {
        *p = [200.0 + yaw_shift, 170.0 + (i as f64 - 27.0) * 5.0];
    }
    pts[30] = [200.0 + yaw_shift, 205.0];
    for (base, x0) in [(36usize, 130.0), (42, 230.0)] {
        pts[base] = [x0, 150.0];
        pts[base + 1] = [x0 + 10.0, 150.0 - eye_open];
        pts[base + 2] = [x0 + 30.0, 150.0 - eye_open];
        pts[base + 3] = [x0 + 40.0, 150.0];
        pts[base + 4] = [x0 + 30.0, 150.0 + eye_open];
        pts[base + 5] = [x0 + 10.0, 150.0 + eye_open];
    }
    for (i, p) in pts.iter_mut().enumerate().skip(48) {
        *p = [170.0 + (i as f64 - 48.0) * 3.0, 240.0];
    }
    json!(pts)
}

fn face(descriptor: &[f64], lms: Option<Value>, size: f64) -> Value {
    json!({"score": 0.98, "box": {"x": 90, "y": 100, "width": size, "height": size},
           "landmarks": lms.unwrap_or_else(|| landmarks(6.0, 0.0)), "descriptor": descriptor})
}

fn image(faces: Vec<Value>) -> Value {
    json!({"width": 640, "height": 480, "faces": faces})
}

fn action_frames(action: &str, base: &[f64]) -> Value {
    let lms = match action {
        "blink" => landmarks(1.6, 0.0),
        "turn_left" => landmarks(6.0, 60.0),
        _ => landmarks(6.0, -60.0),
    };
    json!([
        image(vec![face(&same(base, 1), None, 220.0)]),
        image(vec![face(&same(base, 2), Some(lms), 220.0)]),
        image(vec![face(&same(base, 3), None, 220.0)]),
    ])
}

fn queue(images: Value) {
    QUEUE.lock().unwrap().push_back(images);
}

// ─── people ───────────────────────────────────────────────────────────────────

struct Person {
    token: String,
    employee_id: Uuid,
}

/// `make_person(perms=...)`: a user with an employee record.
async fn make_person(app: &support::TestApp, perms: &[&str]) -> Person {
    let user = app.user_with(perms).await;
    let mut db = app.db().await;
    let emp = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO employees (id, first_name, last_name, email, user_id, is_deleted) VALUES ($1, $2, 'Person', $3, $4, false)",
    )
    .bind(emp)
    .bind(support::u("Pytest"))
    .bind(format!("{emp}@pytest.local"))
    .bind(user.id)
    .execute(&mut db)
    .await
    .unwrap();
    Person {
        token: user.token,
        employee_id: emp,
    }
}

async fn office(app: &support::TestApp) -> (Uuid, String) {
    let mut db = app.db().await;
    sqlx::query("UPDATE office_locations SET is_active = false")
        .execute(&mut db)
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let name = support::u("Pytest HQ");
    sqlx::query(
        "INSERT INTO office_locations (id, name, latitude, longitude, radius_meters, is_active) VALUES ($1, $2, $3, $4, 200, true)",
    )
    .bind(id)
    .bind(&name)
    .bind(rust_decimal::Decimal::try_from(OFFICE.0).unwrap())
    .bind(rust_decimal::Decimal::try_from(OFFICE.1).unwrap())
    .execute(&mut db)
    .await
    .unwrap();
    (id, name)
}

fn photos(n: usize) -> reqwest::multipart::Form {
    let mut form = reqwest::multipart::Form::new();
    for i in 0..n {
        let name = if i == 0 {
            "face_image".to_string()
        } else {
            format!("additional_face_image_{}", i - 1)
        };
        let part = reqwest::multipart::Part::bytes(format!("jpeg-{i}").into_bytes())
            .file_name(format!("p{i}.jpg"))
            .mime_str("image/jpeg")
            .unwrap();
        form = form.part(name, part);
    }
    form
}

async fn post_form(
    app: &support::TestApp,
    token: &str,
    path: &str,
    form: reqwest::multipart::Form,
) -> support::Resp {
    clear_limits(app).await;
    app.send(
        app.client()
            .post(app.url(path))
            .bearer_auth(token)
            .multipart(form),
    )
    .await
}

async fn enroll_with(
    app: &support::TestApp,
    hr: &Person,
    who: &Person,
    base: &[f64],
    shots: usize,
) {
    let imgs: Vec<Value> = (0..shots)
        .map(|i| image(vec![face(&same(base, 50 + i as u64), None, 220.0)]))
        .collect();
    queue(Value::Array(imgs));
    let r = post_form(
        app,
        &hr.token,
        &format!("/api/v1/hr/employees/{}/face/enroll", who.employee_id),
        photos(shots),
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"status": "enrolled", "image_count": shots})
    );
}

fn seed() -> u64 {
    Uuid::new_v4().as_u128() as u64 % 1_000_000_000 * 1000
}

// ─── Enrollment ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_enrollment_stores_descriptors_and_refuses_bad_photos() {
    let app = app();
    let _g = app.serial().await;
    QUEUE.lock().unwrap().clear();
    let run = seed();
    let hr = make_person(app, HR_PERMS).await;
    let (alice, bob) = (make_person(app, &[]).await, make_person(app, &[]).await);
    let a = person(run + 101);
    let url = format!("/api/v1/hr/employees/{}/face/enroll", alice.employee_id);

    queue(json!([image(vec![])]));
    let r = post_form(app, &hr.token, &url, photos(1)).await;
    assert_eq!(r.status, 422);
    assert_eq!(r.json(), json!({"detail": "Photo 1: no face found"}));
    queue(json!([image(vec![
        face(&same(&a, 0), None, 220.0),
        face(&same(&person(run + 102), 0), None, 220.0)
    ])]));
    let r = post_form(app, &hr.token, &url, photos(1)).await;
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .contains("more than one face"));
    queue(json!([
        image(vec![face(&same(&a, 0), None, 220.0)]),
        image(vec![face(&same(&person(run + 103), 0), None, 220.0)])
    ]));
    let r = post_form(app, &hr.token, &url, photos(2)).await;
    assert_eq!(
        r.json(),
        json!({"detail": "Photos 1 and 2 do not look like the same person"})
    );
    queue(json!([image(vec![face(&same(&a, 0), None, 100.0)])]));
    let r = post_form(app, &hr.token, &url, photos(1)).await;
    assert!(r.json()["detail"].as_str().unwrap().contains("too small"));

    enroll_with(app, &hr, &alice, &a, 2).await;
    let mut db = app.db().await;
    let rows: Vec<Vec<f64>> =
        sqlx::query_scalar("SELECT descriptor FROM face_descriptors WHERE employee_id = $1")
            .bind(alice.employee_id)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert!(rows.len() == 2 && rows[0].len() == 128);
    let (count, active): (Option<i32>, Option<bool>) = sqlx::query_as(
        "SELECT image_count, is_active FROM face_enrollments WHERE employee_id = $1",
    )
    .bind(alice.employee_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((count, active), (Some(2), Some(true)));

    // Bob enrolled with Alice's face: refused as a duplicate identity
    queue(json!([image(vec![face(&same(&a, 77), None, 220.0)])]));
    let dup = post_form(
        app,
        &hr.token,
        &format!("/api/v1/hr/employees/{}/face/enroll", bob.employee_id),
        photos(1),
    )
    .await;
    assert_eq!(dup.status, 409);
    assert!(dup.json()["detail"]
        .as_str()
        .unwrap()
        .starts_with("This face is already enrolled for "));
    // re-enrolling Alice replaces her descriptors
    enroll_with(app, &hr, &alice, &a, 1).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM face_descriptors WHERE employee_id = $1")
        .bind(alice.employee_id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

// ─── Personal device ──────────────────────────────────────────────────────────

struct Clock<'a> {
    event_type: &'a str,
    lat: f64,
    lng: f64,
    accuracy: f64,
    nonce: Option<String>,
    same_frames: bool,
}

impl Default for Clock<'_> {
    fn default() -> Self {
        Clock {
            event_type: "clock_in",
            lat: OFFICE.0,
            lng: OFFICE.1,
            accuracy: 20.0,
            nonce: None,
            same_frames: false,
        }
    }
}

async fn personal_clock(
    app: &support::TestApp,
    token: &str,
    frames_for: Option<&dyn Fn(&str) -> Value>,
    c: Clock<'_>,
) -> (support::Resp, String) {
    let nonce = match c.nonce {
        Some(n) => n,
        None => {
            clear_limits(app).await;
            let ch = app
                .post(
                    token,
                    "/api/v1/hr/attendance/challenge",
                    json!({"event_type": c.event_type}),
                )
                .await;
            assert_eq!(ch.status, 200, "{}", ch.text());
            let ch = ch.json();
            if let Some(f) = frames_for {
                queue(f(ch["action"].as_str().unwrap()));
            }
            ch["nonce"].as_str().unwrap().to_string()
        }
    };
    let mut form = reqwest::multipart::Form::new()
        .text("nonce", nonce.clone())
        .text("latitude", c.lat.to_string())
        .text("longitude", c.lng.to_string())
        .text("geo_accuracy_meters", c.accuracy.to_string());
    for i in 0..3 {
        let bytes = if c.same_frames {
            b"same-bytes".to_vec()
        } else {
            format!("frame-{i}-{nonce}").into_bytes()
        };
        form = form.part(
            format!("frame_{i}"),
            reqwest::multipart::Part::bytes(bytes)
                .file_name(format!("f{i}.jpg"))
                .mime_str("image/jpeg")
                .unwrap(),
        );
    }
    let path = format!("/api/v1/hr/attendance/{}", c.event_type.replace('_', "-"));
    (post_form(app, token, &path, form).await, nonce)
}

#[tokio::test]
async fn test_personal_clock_in_needs_login_liveness_gps_and_your_own_face() {
    let app = app();
    let _g = app.serial().await;
    QUEUE.lock().unwrap().clear();
    let run = seed();
    let (_, office_name) = office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let (me, other) = (make_person(app, &[]).await, make_person(app, &[]).await);
    let (mine, theirs) = (person(run + 201), person(run + 202));
    enroll_with(app, &hr, &me, &mine, 2).await;
    enroll_with(app, &hr, &other, &theirs, 2).await;

    let r = app
        .post(
            "",
            "/api/v1/hr/attendance/challenge",
            json!({"event_type": "clock_in"}),
        )
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(
        app.request("POST", "/api/v1/hr/attendance/clock-in", "", None)
            .await
            .status,
        401
    );

    // Someone else's face on my account
    let f = |a: &str| action_frames(a, &theirs);
    let (r, _) = personal_clock(app, &me.token, Some(&f), Clock::default()).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(r.json()["detail"]["code"], "face_mismatch");
    // A still photo cannot perform the action
    let still = |_: &str| {
        json!([
            image(vec![face(&same(&mine, 0), None, 220.0)]),
            image(vec![face(&same(&mine, 1), None, 220.0)]),
            image(vec![face(&same(&mine, 2), None, 220.0)])
        ])
    };
    let (r, _) = personal_clock(app, &me.token, Some(&still), Clock::default()).await;
    assert_eq!(r.status, 401, "{}", r.text());
    assert_eq!(r.json()["detail"]["code"], "liveness_failed");
    // GPS: outside the fence, 0,0, too imprecise
    let ok = |a: &str| action_frames(a, &mine);
    for (c, text) in [
        (
            Clock {
                lat: -33.86,
                lng: 151.2,
                ..Default::default()
            },
            "must be within",
        ),
        (
            Clock {
                lat: 0.0,
                lng: 0.0,
                ..Default::default()
            },
            "not valid",
        ),
        (
            Clock {
                accuracy: 900.0,
                ..Default::default()
            },
            "too imprecise",
        ),
    ] {
        let (r, _) = personal_clock(app, &me.token, Some(&ok), c).await;
        assert_eq!(r.status, 400, "{}", r.text());
        assert!(
            r.json()["detail"].as_str().unwrap().contains(text),
            "{}",
            r.text()
        );
        QUEUE.lock().unwrap().clear();
    }
    // The same image three times
    let (r, _) = personal_clock(
        app,
        &me.token,
        Some(&ok),
        Clock {
            same_frames: true,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(r.status, 400);
    assert!(r.json()["detail"].as_str().unwrap().contains("identical"));
    QUEUE.lock().unwrap().clear();

    // My own live face: clocked in
    let (r, nonce) = personal_clock(app, &me.token, Some(&ok), Clock::default()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    assert_eq!(body["status"], "success");
    assert_eq!(body["office_location"], json!(office_name));
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "status",
            "event_type",
            "employee_name",
            "time",
            "late",
            "message",
            "office_location"
        ]
    );
    // The challenge is single use
    let (r, _) = personal_clock(
        app,
        &me.token,
        None,
        Clock {
            nonce: Some(nonce),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(r.status, 400);
    assert!(r.json()["detail"].as_str().unwrap().contains("expired"));
    // A second clock-in the same day is refused, and still logged
    let (r, _) = personal_clock(app, &me.token, Some(&ok), Clock::default()).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.json()["detail"]["code"], "already_clocked_in");
    let mut db = app.db().await;
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT recognition_status FROM clock_events WHERE employee_id = $1")
            .bind(me.employee_id)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert!(
        statuses.contains(&"matched".to_string()) && statuses.contains(&"duplicate".to_string())
    );
    let (lat, details): (Option<rust_decimal::Decimal>, Option<Value>) = sqlx::query_as(
        "SELECT latitude, details FROM clock_events WHERE employee_id = $1 AND recognition_status = 'matched'",
    )
    .bind(me.employee_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(lat.unwrap().to_string(), "-45.0311000");
    assert!(details.unwrap()["liveness"]["ear"].is_array());
}

#[tokio::test]
async fn test_weak_personal_match_goes_to_hr_review() {
    let app = app();
    let _g = app.serial().await;
    QUEUE.lock().unwrap().clear();
    let run = seed();
    office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let me = make_person(app, &[]).await;
    let mine = person(run + 301);
    enroll_with(app, &hr, &me, &mine, 2).await;
    // drift of length 0.53: ~0.55 from the enrolled shots, above 0.50, inside the review band
    let dir = person(run + 302);
    let norm = dir.iter().map(|x| x * x).sum::<f64>().sqrt();
    let drifted: Vec<f64> = mine
        .iter()
        .zip(&dir)
        .map(|(m, d)| m + d / norm * 0.53)
        .collect();
    let f = |a: &str| action_frames(a, &drifted);
    let (r, _) = personal_clock(app, &me.token, Some(&f), Clock::default()).await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(r.json()["status"], "pending_override");
    let mut db = app.db().await;
    let (event, status): (Uuid, String) = sqlx::query_as(
        "SELECT id, recognition_status FROM clock_events WHERE employee_id = $1 ORDER BY captured_at DESC LIMIT 1",
    )
    .bind(me.employee_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(status, "pending_override");
    let types: Vec<String> =
        sqlx::query_scalar("SELECT type FROM notifications WHERE entity_id = $1")
            .bind(event.to_string())
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert!(!types.is_empty() && types.iter().all(|t| t == "attendance_review"));
}

// ─── Kiosk ────────────────────────────────────────────────────────────────────

async fn kiosk(app: &support::TestApp, hr: &Person, office_id: Uuid) -> (String, String) {
    let r = app
        .post(
            &hr.token,
            "/api/v1/hr/kiosks",
            json!({"name": support::u("Lobby tablet"), "office_location_id": office_id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    clear_limits(app).await;
    let token = r.json()["token"].as_str().unwrap().to_string();
    let s = app
        .send(
            app.client()
                .post(app.url("/api/v1/hr/kiosk/session"))
                .header("X-Kiosk-Token", token),
        )
        .await;
    assert_eq!(s.status, 200, "{}", s.text());
    (
        r.json()["id"].as_str().unwrap().to_string(),
        s.json()["session"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn test_engine_and_gallery_only_after_kiosk_authentication() {
    let app = app();
    let _g = app.serial().await;
    QUEUE.lock().unwrap().clear();
    let run = seed();
    let (office_id, _) = office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let staff = make_person(app, &[]).await;
    enroll_with(app, &hr, &staff, &person(run + 401), 2).await;
    let (kiosk_id, session) = kiosk(app, &hr, office_id).await;

    assert_eq!(
        app.get("", "/api/v1/hr/kiosk/engine/face-api.esm.js")
            .await
            .status,
        401
    );
    assert_eq!(app.get("", "/api/v1/hr/kiosk/gallery").await.status, 401);
    clear_limits(app).await;
    let forged = app
        .send(
            app.client()
                .post(app.url("/api/v1/hr/kiosk/session"))
                .header("X-Kiosk-Token", "kiosk_forged"),
        )
        .await;
    assert_eq!(forged.status, 401);
    // a user token is not a kiosk session
    assert_eq!(
        app.get(&staff.token, "/api/v1/hr/kiosk/gallery")
            .await
            .status,
        401
    );
    let r = app
        .get(
            &staff.token,
            "/api/v1/hr/attendance/engine/face_recognition_model-weights_manifest.json",
        )
        .await;
    assert_eq!(r.status, 404);
    let r = app
        .get(&staff.token, "/api/v1/hr/attendance/engine/face-api.esm.js")
        .await;
    assert_eq!(
        (r.status, r.body.as_slice()),
        (200, &b"export const faceapi = {};"[..])
    );

    let engine_file = app
        .get(&session, "/api/v1/hr/kiosk/engine/face-api.esm.js")
        .await;
    assert_eq!(engine_file.status, 200);
    assert_eq!(
        engine_file.header("cache-control").as_deref(),
        Some("private, no-store")
    );
    assert_eq!(
        engine_file.header("content-type").as_deref(),
        Some("application/octet-stream")
    );
    let r = app
        .get(
            &session,
            "/api/v1/hr/kiosk/engine/face_recognition_model-weights_manifest.json",
        )
        .await;
    assert_eq!((r.status, r.text().as_str()), (200, "[]"));
    assert_eq!(
        app.get(&session, "/api/v1/hr/kiosk/engine/..%2F..%2Fpackage.json")
            .await
            .status,
        404
    );
    let gallery = app.get(&session, "/api/v1/hr/kiosk/gallery").await;
    assert_eq!(gallery.status, 200);
    let body = gallery.json();
    let mine: Vec<&Value> = body["employees"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["employee_id"] == json!(staff.employee_id.to_string()))
        .collect();
    assert!(!mine.is_empty());
    assert_eq!(mine[0]["descriptors"][0].as_array().unwrap().len(), 128);
    let etag = gallery.header("etag").unwrap();
    assert_eq!(json!(etag), body["version"]);
    let again = app
        .send(
            app.client()
                .get(app.url("/api/v1/hr/kiosk/gallery"))
                .bearer_auth(&session)
                .header("If-None-Match", &etag),
        )
        .await;
    assert_eq!(again.status, 304);
    assert!(again.body.is_empty());

    assert_eq!(
        app.post(
            &hr.token,
            &format!("/api/v1/hr/kiosks/{kiosk_id}/revoke"),
            json!({})
        )
        .await
        .status,
        200
    );
    // revocation cuts off a live session immediately
    assert_eq!(
        app.get(&session, "/api/v1/hr/kiosk/gallery").await.status,
        401
    );
}

#[tokio::test]
async fn test_kiosk_clock_is_revalidated_by_the_servers_own_search() {
    let app = app();
    let _g = app.serial().await;
    QUEUE.lock().unwrap().clear();
    let run = seed();
    let (office_id, _) = office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let (ann, ben) = (make_person(app, &[]).await, make_person(app, &[]).await);
    let (a, b) = (person(run + 501), person(run + 502));
    enroll_with(app, &hr, &ann, &a, 2).await;
    enroll_with(app, &hr, &ben, &b, 2).await;
    let (_, session) = kiosk(app, &hr, office_id).await;
    CALLS.store(0, Ordering::SeqCst);

    let body = |employee: Uuid, descriptor: Vec<f64>, liveness: Value| {
        json!({"event_type": "clock_in", "employee_id": employee.to_string(), "descriptor": descriptor,
               "distance": 0.2, "action": "blink", "liveness": liveness,
               "captured_at": chrono::Utc::now().to_rfc3339()})
    };
    let live = json!({"passed": true, "metrics": {"ear": [0.3, 0.1, 0.3]}});
    let s = session.as_str();
    let clock = |b: Value| async move {
        clear_limits(app).await;
        app.post(s, "/api/v1/hr/kiosk/clock", b).await
    };
    // The kiosk claims Ben, but the embedding is Ann's: the server's own search disagrees
    let mismatch = clock(body(ben.employee_id, same(&a, 9), live.clone())).await;
    assert_eq!(mismatch.status, 422, "{}", mismatch.text());
    assert_eq!(mismatch.json()["detail"]["code"], "identity_unconfirmed");
    let stranger = clock(body(
        ann.employee_id,
        same(&person(run + 503), 0),
        live.clone(),
    ))
    .await;
    assert_eq!(stranger.status, 422);
    let no_live = clock(body(ann.employee_id, same(&a, 9), json!({"passed": false}))).await;
    assert_eq!(no_live.status, 422);
    assert_eq!(no_live.json()["detail"]["code"], "liveness_failed");
    let ok = clock(body(ann.employee_id, same(&a, 9), live.clone())).await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert!(ok.json()["employee_name"]
        .as_str()
        .is_some_and(|n| !n.is_empty()));
    assert_eq!(
        CALLS.load(Ordering::SeqCst),
        0,
        "kiosk clock-ins never touch the image engine"
    );

    let mut db = app.db().await;
    type EventRow = (String, Value, bool, Option<f64>, Option<Uuid>);
    let events: Vec<EventRow> = sqlx::query_as(
        "SELECT recognition_status, details, is_kiosk, match_distance, office_location_id FROM clock_events \
         WHERE details->>'kiosk_claim' = ANY($1)",
    )
    .bind(vec![ann.employee_id.to_string(), ben.employee_id.to_string()])
    .fetch_all(&mut db)
    .await
    .unwrap();
    let mism = events.iter().find(|e| e.0 == "kiosk_mismatch").unwrap();
    assert_eq!(mism.1["server_match"], json!(ann.employee_id.to_string()));
    assert_eq!(mism.1["kiosk_claim"], json!(ben.employee_id.to_string()));
    let matched = events.iter().find(|e| e.0 == "matched").unwrap();
    assert!(matched.2 && matched.3.unwrap() < 0.5 && matched.4 == Some(office_id));
}

#[tokio::test]
async fn test_rate_limit_answers_429() {
    let app = app();
    let _g = app.serial().await;
    clear_limits(app).await;
    let staff = make_person(app, &[]).await;
    for _ in 0..10 {
        app.post(
            &staff.token,
            "/api/v1/hr/attendance/challenge",
            json!({"event_type": "clock_in"}),
        )
        .await;
    }
    let r = app
        .post(
            &staff.token,
            "/api/v1/hr/attendance/challenge",
            json!({"event_type": "clock_in"}),
        )
        .await;
    assert_eq!(r.status, 429);
    assert_eq!(
        r.json(),
        json!({"detail": "Too many attempts: wait a minute and try again"})
    );
    clear_limits(app).await;
}

// ─── Face check telemetry and step budgets (migration 0165) ───────────────────

async fn summary(app: &support::TestApp, token: &str) -> Value {
    let r = app
        .get(token, "/api/v1/hr/attendance/face-attempts/summary?days=7")
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

fn n(v: &Value, path: &[&str]) -> i64 {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_i64().unwrap_or(0)
}

#[tokio::test]
async fn test_challenge_and_kiosk_session_carry_the_step_budgets() {
    let app = app();
    let _g = app.serial().await;
    let (office_id, _) = office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let me = make_person(app, &[]).await;
    enroll_with(app, &hr, &me, &person(seed() + 601), 2).await;

    clear_limits(app).await;
    let ch = app
        .post(
            &me.token,
            "/api/v1/hr/attendance/challenge",
            json!({"event_type": "clock_in"}),
        )
        .await;
    assert_eq!(ch.status, 200, "{}", ch.text());
    let ch = ch.json();
    assert_eq!(
        ch["step_timeouts_ms"],
        json!({"find": 8000, "action": 10000, "return": 5000})
    );
    // the nonce outlives the steps (23 s) plus the upload margin: the setting's 60 s
    assert_eq!(ch["expires_in"], json!(60));
    assert!(ch["nonce"].as_str().is_some_and(|s| !s.is_empty()));

    let r = app
        .post(
            &hr.token,
            "/api/v1/hr/kiosks",
            json!({"name": support::u("Budget tablet"), "office_location_id": office_id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let device_token = r.json()["token"].as_str().unwrap().to_string();
    clear_limits(app).await;
    let s = app
        .send(
            app.client()
                .post(app.url("/api/v1/hr/kiosk/session"))
                .header("X-Kiosk-Token", device_token),
        )
        .await;
    assert_eq!(s.status, 200, "{}", s.text());
    assert_eq!(
        s.json()["step_timeouts_ms"],
        json!({"find": 8000, "action": 10000, "return": 5000})
    );

    // A TTL shorter than the steps is raised to cover them; a missing setting names its key.
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE system_settings SET value = '10' WHERE key = 'hr.face.challenge_ttl_seconds'",
    )
    .execute(&mut db)
    .await
    .unwrap();
    clear_limits(app).await;
    let short = app
        .post(
            &me.token,
            "/api/v1/hr/attendance/challenge",
            json!({"event_type": "clock_in"}),
        )
        .await;
    assert_eq!(short.json()["expires_in"], json!(23 + 15));
    sqlx::query("UPDATE system_settings SET value = '' WHERE key = 'hr.face.step_timeouts_ms'")
        .execute(&mut db)
        .await
        .unwrap();
    clear_limits(app).await;
    let missing = app
        .post(
            &me.token,
            "/api/v1/hr/attendance/challenge",
            json!({"event_type": "clock_in"}),
        )
        .await;
    sqlx::query(
        "UPDATE system_settings SET value = CASE key WHEN 'hr.face.step_timeouts_ms' \
         THEN '{\"find\": 8000, \"action\": 10000, \"return\": 5000}' ELSE '60' END \
         WHERE key IN ('hr.face.step_timeouts_ms', 'hr.face.challenge_ttl_seconds')",
    )
    .execute(&mut db)
    .await
    .unwrap();
    assert_eq!(missing.status, 400, "{}", missing.text());
    assert!(missing.json()["detail"]
        .as_str()
        .unwrap()
        .contains("hr.face.step_timeouts_ms"));
    clear_limits(app).await;
}

#[tokio::test]
async fn test_face_attempts_are_recorded_validated_limited_and_summarised() {
    let app = app();
    let _g = app.serial().await;
    let (office_id, _) = office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let staff = make_person(app, &[]).await;
    let (kiosk_id, session) = kiosk(app, &hr, office_id).await;
    clear_limits(app).await;
    let before = summary(app, &hr.token).await;

    let report = |outcome: &str, step: &str| {
        json!({"action": "turn_left", "outcome": outcome, "step_reached": step, "duration_ms": 12400,
               "avg_tick_ms": 320, "backend": "webgl"})
    };
    // personal: a logged-in user without an employee check, linked to their employee row
    let r = app
        .send(
            app.client()
                .post(app.url("/api/v1/hr/attendance/face-attempts"))
                .bearer_auth(&staff.token)
                .header("User-Agent", "x".repeat(300))
                .json(&report("timeout", "do_action")),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let personal_id: Uuid = r.json()["id"].as_str().unwrap().parse().unwrap();
    // kiosk: the tablet's session, with the employee it recognised
    let mut done = report("done", "done");
    done["employee_id"] = json!(staff.employee_id.to_string());
    done["backend"] = json!("cpu");
    // 0167: what went wrong on the device and the tick numbers
    done["note"] = json!("stall,cpu_fallback");
    done["first_tick_ms"] = json!(3100);
    done["ticks"] = json!(41);
    let k = app
        .post(&session, "/api/v1/hr/kiosk/face-attempts", done)
        .await;
    assert_eq!(k.status, 201, "{}", k.text());
    let kiosk_row: Uuid = k.json()["id"].as_str().unwrap().parse().unwrap();

    let mut db = app.db().await;
    type Row = (
        String,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        String,
        i32,
        Option<i32>,
        String,
        Option<String>,
    );
    let p: Row = sqlx::query_as(
        "SELECT mode, kiosk_device_id, user_id, employee_id, outcome, duration_ms, avg_tick_ms, backend, user_agent \
         FROM face_check_attempts WHERE id = $1",
    )
    .bind(personal_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(p.0, "personal");
    assert_eq!(p.1, None);
    assert!(p.2.is_some());
    assert_eq!(p.3, Some(staff.employee_id));
    assert_eq!((p.4.as_str(), p.5, p.6), ("timeout", 12400, Some(320)));
    assert_eq!(p.8.as_deref().map(str::len), Some(200));
    let kr: Row = sqlx::query_as(
        "SELECT mode, kiosk_device_id, user_id, employee_id, outcome, duration_ms, avg_tick_ms, backend, user_agent \
         FROM face_check_attempts WHERE id = $1",
    )
    .bind(kiosk_row)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(kr.0, "kiosk");
    assert_eq!(kr.1.map(|u| u.to_string()), Some(kiosk_id.clone()));
    assert_eq!((kr.2, kr.3), (None, Some(staff.employee_id)));
    assert_eq!(kr.7, "cpu");
    let (note, details): (Option<String>, Option<Value>) =
        sqlx::query_as("SELECT note, details FROM face_check_attempts WHERE id = $1")
            .bind(kiosk_row)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(note.as_deref(), Some("stall,cpu_fallback"));
    assert_eq!(details, Some(json!({"first_tick_ms": 3100, "ticks": 41})));
    let (note, details): (Option<String>, Option<Value>) =
        sqlx::query_as("SELECT note, details FROM face_check_attempts WHERE id = $1")
            .bind(personal_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((note, details), (None, None));
    // a kiosk passive check that fell back reports why, with its numbers
    let k = app
        .post(
            &session,
            "/api/v1/hr/kiosk/face-attempts",
            json!({"action": "turn_right", "outcome": "passive_failed", "step_reached": "done",
                   "duration_ms": 2600, "avg_tick_ms": null, "backend": "webgl", "reason": "flash_failed",
                   "corr": 0.21, "delta": 2.4, "spoof_real": 0.91, "lag": 1}),
        )
        .await;
    assert_eq!(k.status, 201, "{}", k.text());
    let kp: Uuid = k.json()["id"].as_str().unwrap().parse().unwrap();
    let (details,): (Option<Value>,) =
        sqlx::query_as("SELECT details FROM face_check_attempts WHERE id = $1")
            .bind(kp)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        details,
        Some(
            json!({"reason": "flash_failed", "corr": 0.21, "delta": 2.4, "spoof_real": 0.91, "lag": 1})
        )
    );

    // validation
    for bad in [
        json!({"action": "nod", "outcome": "done", "step_reached": "done", "duration_ms": 1}),
        json!({"action": "blink", "outcome": "crashed", "step_reached": "done", "duration_ms": 1}),
        json!({"action": "blink", "outcome": "done", "step_reached": "matching", "duration_ms": 1}),
        json!({"action": "blink", "outcome": "done", "step_reached": "done", "duration_ms": -5}),
        json!({"action": "blink", "outcome": "done", "step_reached": "done", "duration_ms": 1, "backend": "wasm"}),
        json!({"action": "blink", "outcome": "done", "step_reached": "done"}),
        json!({"action": "turn_left", "outcome": "passive_failed", "step_reached": "done", "duration_ms": 1,
               "reason": "bored"}),
        json!({"action": "turn_left", "outcome": "done", "step_reached": "done", "duration_ms": 1,
               "note": "x".repeat(201)}),
        json!({"action": "turn_left", "outcome": "done", "step_reached": "done", "duration_ms": 1, "corr": 3}),
    ] {
        let r = app
            .post(
                &staff.token,
                "/api/v1/hr/attendance/face-attempts",
                bad.clone(),
            )
            .await;
        assert_eq!(r.status, 422, "{bad} -> {}", r.text());
    }
    // auth: no token, a user token on the kiosk route, a kiosk session on the personal route
    assert_eq!(
        app.post(
            "",
            "/api/v1/hr/attendance/face-attempts",
            report("done", "done")
        )
        .await
        .status,
        401
    );
    assert_eq!(
        app.post(
            &staff.token,
            "/api/v1/hr/kiosk/face-attempts",
            report("done", "done")
        )
        .await
        .status,
        401
    );

    // summary: HR only, counts by outcome, mode and step
    assert_eq!(
        app.get(&staff.token, "/api/v1/hr/attendance/face-attempts/summary")
            .await
            .status,
        403
    );
    assert_eq!(
        app.get(
            &hr.token,
            "/api/v1/hr/attendance/face-attempts/summary?days=0"
        )
        .await
        .status,
        422
    );
    let after = summary(app, &hr.token).await;
    assert_eq!(after["days"], json!(7));
    assert_eq!(n(&after, &["total"]) - n(&before, &["total"]), 3);
    assert_eq!(
        n(&after, &["passive", "reasons", "flash_failed"])
            - n(&before, &["passive", "reasons", "flash_failed"]),
        1
    );
    assert!(after["passive"]["median_corr"].is_number(), "{after}");
    assert!(after["passive"]["passed"].is_number());
    assert_eq!(n(&after, &["timeouts"]) - n(&before, &["timeouts"]), 1);
    assert_eq!(
        n(&after, &["by_outcome", "timeout"]) - n(&before, &["by_outcome", "timeout"]),
        1
    );
    assert_eq!(
        n(&after, &["by_mode", "kiosk"]) - n(&before, &["by_mode", "kiosk"]),
        2
    );
    assert_eq!(
        n(&after, &["by_step", "do_action"]) - n(&before, &["by_step", "do_action"]),
        1
    );
    assert_eq!(
        n(&after, &["by_mode_outcome", "personal", "timeout"])
            - n(&before, &["by_mode_outcome", "personal", "timeout"]),
        1
    );
    assert!(after["timeout_rate"].as_f64().unwrap() > 0.0);

    // rate limit: 30 a minute per user
    clear_limits(app).await;
    let mut last = 0;
    for _ in 0..31 {
        last = app
            .post(
                &staff.token,
                "/api/v1/hr/attendance/face-attempts",
                report("aborted", "hold_still"),
            )
            .await
            .status;
    }
    assert_eq!(last, 429);
    sqlx::query("DELETE FROM face_check_attempts WHERE user_id = (SELECT user_id FROM employees WHERE id = $1)")
        .bind(staff.employee_id)
        .execute(&mut db)
        .await
        .unwrap();
    clear_limits(app).await;
}

// ─── Passive liveness (migration 0166) ────────────────────────────────────────

const NEUTRAL_MEAN: [f64; 3] = [150.0, 110.0, 90.0];

/// The engine's `/passive` answer: the neutral frame (a face with a descriptor, its cheek colour and the
/// anti-spoof score) and one flash frame per colour whose cheek colour moved by `gain` x the colour.
fn passive_answer(base: &[f64], colors: &[[f64; 3]], gain: f64, spoof_real: f64) -> Value {
    let mean = |c: [f64; 3]| -> Value {
        json!([
            NEUTRAL_MEAN[0] + gain * c[0],
            NEUTRAL_MEAN[1] + gain * c[1],
            NEUTRAL_MEAN[2] + gain * c[2]
        ])
    };
    let mut neutral = image(vec![face(&same(base, 1), None, 220.0)]);
    neutral["face"] = json!(true);
    neutral["region_mean"] = mean([0.0; 3]);
    neutral["spoof_real"] = json!(spoof_real);
    let flashes: Vec<Value> = colors
        .iter()
        .map(|c| {
            let mut f = image(vec![face(&[], None, 220.0)]);
            f["face"] = json!(true);
            f["region_mean"] = mean(*c);
            f
        })
        .collect();
    json!({"neutral": neutral, "flashes": flashes})
}

fn colors_of(ch: &Value) -> Vec<[f64; 3]> {
    ch["passive"]["colors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let a = c.as_array().unwrap();
            [
                a[0].as_f64().unwrap(),
                a[1].as_f64().unwrap(),
                a[2].as_f64().unwrap(),
            ]
        })
        .collect()
}

async fn challenge_for(app: &support::TestApp, token: &str) -> Value {
    clear_limits(app).await;
    let ch = app
        .post(
            token,
            "/api/v1/hr/attendance/challenge",
            json!({"event_type": "clock_in"}),
        )
        .await;
    assert_eq!(ch.status, 200, "{}", ch.text());
    ch.json()
}

/// A passive clock-in: frame_neutral + flash_0..flash_{k-1}, mode=passive.
async fn passive_clock(
    app: &support::TestApp,
    token: &str,
    nonce: &str,
    flashes: usize,
) -> support::Resp {
    let part = |b: String| {
        reqwest::multipart::Part::bytes(b.into_bytes())
            .file_name("f.jpg")
            .mime_str("image/jpeg")
            .unwrap()
    };
    let mut form = reqwest::multipart::Form::new()
        .text("nonce", nonce.to_string())
        .text("mode", "passive")
        .text("latitude", OFFICE.0.to_string())
        .text("longitude", OFFICE.1.to_string())
        .text("geo_accuracy_meters", "20")
        .part("frame_neutral", part(format!("neutral-{nonce}")));
    for i in 0..flashes {
        form = form.part(format!("flash_{i}"), part(format!("flash-{i}-{nonce}")));
    }
    post_form(app, token, "/api/v1/hr/attendance/clock-in", form).await
}

type EventRow = (String, Option<f64>, Option<String>, Value);

async fn latest_event(app: &support::TestApp, employee: Uuid) -> EventRow {
    let mut db = app.db().await;
    sqlx::query_as(
        "SELECT recognition_status, liveness_score, liveness_challenge, details FROM clock_events \
         WHERE employee_id = $1 ORDER BY captured_at DESC LIMIT 1",
    )
    .bind(employee)
    .fetch_one(&mut db)
    .await
    .unwrap()
}

async fn attempts_of(app: &support::TestApp, emp: Uuid) -> Vec<(String, String, String)> {
    let mut db = app.db().await;
    sqlx::query_as(
        "SELECT mode, outcome, action FROM face_check_attempts WHERE employee_id = $1 ORDER BY created_at",
    )
    .bind(emp)
    .fetch_all(&mut db)
    .await
    .unwrap()
}

#[tokio::test]
async fn test_passive_check_passes_and_clocks_in() {
    let app = app();
    let _g = app.serial().await;
    PASSIVE_QUEUE.lock().unwrap().clear();
    let run = seed();
    office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let me = make_person(app, &[]).await;
    let mine = person(run + 701);
    enroll_with(app, &hr, &me, &mine, 2).await;

    let ch = challenge_for(app, &me.token).await;
    let p = &ch["passive"];
    assert_eq!(p["enabled"], json!(true));
    assert_eq!(
        (p["flash_ms"].clone(), p["settle_ms"].clone()),
        (json!(400), json!(340))
    );
    let colors = colors_of(&ch);
    assert_eq!(colors.len(), 5);
    assert!(colors.windows(2).all(|w| w[0] != w[1]), "{colors:?}");
    // the active fields are still there for older phones
    assert!(ch["action"].as_str().is_some() && ch["step_timeouts_ms"].is_object());

    PASSIVE_QUEUE
        .lock()
        .unwrap()
        .push_back(passive_answer(&mine, &colors, 0.5, 0.93));
    let r = passive_clock(app, &me.token, ch["nonce"].as_str().unwrap(), 5).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "success", "{}", r.text());
    assert_eq!(
        PASSIVE_FIELDS.lock().unwrap().clone(),
        [
            "flash_0",
            "flash_1",
            "flash_2",
            "flash_3",
            "flash_4",
            "frame_neutral"
        ]
    );
    let (status, score, challenge, details) = latest_event(app, me.employee_id).await;
    assert_eq!(status, "matched");
    assert_eq!(score, Some(0.93));
    assert_eq!(challenge.as_deref(), Some("passive"));
    assert_eq!(details["passive"]["method"], "passive");
    assert_eq!(details["passive"]["flash"], "pass");
    assert!(details["passive"]["corr"].as_f64().unwrap() > 0.99);
    assert!(details["passive"]["delta"].as_f64().unwrap() > 1.5);
    // 0167 diagnostics: lag, faces per frame (neutral first), frame size, flash timing
    assert_eq!(details["passive"]["lag"], json!(0));
    assert_eq!(details["passive"]["faces"], json!([1, 1, 1, 1, 1, 1]));
    assert!(details["passive"]["frame"].is_array());
    assert_eq!(details["passive"]["flash_ms"], json!(400));
    // HR's clock events list carries the method and the score
    let list = app
        .get(
            &hr.token,
            &format!(
                "/api/v1/hr/attendance/clock-events?employee_id={}",
                me.employee_id
            ),
        )
        .await;
    assert_eq!(list.status, 200, "{}", list.text());
    let first = &list.json()[0];
    assert_eq!(first["details"]["passive"]["method"], "passive");
    assert_eq!(first["liveness_score"], json!(0.93));
    // missing a flash frame: refused before the engine is asked
    let ch = challenge_for(app, &me.token).await;
    let r = passive_clock(app, &me.token, ch["nonce"].as_str().unwrap(), 4).await;
    assert_eq!(r.status, 400, "{}", r.text());
    clear_limits(app).await;
}

#[tokio::test]
async fn test_passive_not_sure_falls_back_to_the_active_check() {
    let app = app();
    let _g = app.serial().await;
    PASSIVE_QUEUE.lock().unwrap().clear();
    QUEUE.lock().unwrap().clear();
    let run = seed();
    office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let (me, other) = (make_person(app, &[]).await, make_person(app, &[]).await);
    let (mine, theirs) = (person(run + 801), person(run + 802));
    enroll_with(app, &hr, &me, &mine, 2).await;
    enroll_with(app, &hr, &other, &theirs, 2).await;

    // no colour response (bright light, or a still picture): not sure -> the active check
    let ch = challenge_for(app, &me.token).await;
    let colors = colors_of(&ch);
    PASSIVE_QUEUE
        .lock()
        .unwrap()
        .push_back(passive_answer(&mine, &colors, 0.0, 0.95));
    let r = passive_clock(app, &me.token, ch["nonce"].as_str().unwrap(), colors.len()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    assert_eq!(body["active_required"], json!(true));
    assert_eq!(body["reason"], "flash_inconclusive");
    let next = &body["challenge"];
    let mut keys: Vec<&String> = next.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["action", "expires_in", "nonce", "step_timeouts_ms"]);
    assert_ne!(next["nonce"], ch["nonce"]);
    let rows = attempts_of(app, me.employee_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1.as_str()),
        ("personal", "passive_inconclusive")
    );
    assert_eq!(json!(rows[0].2), next["action"]);
    // only head turns are asked for now
    assert_ne!(next["action"], "blink");
    // the server-written row carries why and the numbers (0167), no images
    let mut db = app.db().await;
    let (note, d): (Option<String>, Option<Value>) = sqlx::query_as(
        "SELECT note, details FROM face_check_attempts WHERE employee_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(me.employee_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    drop(db);
    assert_eq!(note, None);
    let d = d.expect("details");
    assert_eq!(d["reason"], "flash_inconclusive");
    assert_eq!(d["flash"], "inconclusive");
    assert_eq!(d["spoof_real"], json!(0.95));
    assert_eq!(d["faces"], json!([1, 1, 1, 1, 1, 1]));
    assert!(d["corr"].is_number() && d["delta"].is_number() && d["lag"].is_number());
    // the passive nonce is spent
    let r = passive_clock(app, &me.token, ch["nonce"].as_str().unwrap(), colors.len()).await;
    assert_eq!(r.status, 400);

    // the active check with the new nonce (no mode field) clocks in, and keeps why
    let action = next["action"].as_str().unwrap().to_string();
    queue(action_frames(&action, &mine));
    let (r, _) = personal_clock(
        app,
        &me.token,
        None,
        Clock {
            nonce: Some(next["nonce"].as_str().unwrap().to_string()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (status, score, challenge, details) = latest_event(app, me.employee_id).await;
    assert_eq!(status, "matched");
    assert_eq!(challenge.as_deref(), Some(action.as_str()));
    assert_eq!(details["passive"]["method"], "active");
    assert_eq!(details["passive"]["fallback_reason"], "flash_inconclusive");
    assert_eq!(score, Some(0.95));
    assert!(details["liveness"]["ear"].is_array());

    // the colours followed but the anti-spoof model says it is not a real face: failed -> active
    let ch = challenge_for(app, &other.token).await;
    let colors = colors_of(&ch);
    PASSIVE_QUEUE
        .lock()
        .unwrap()
        .push_back(passive_answer(&theirs, &colors, 0.5, 0.2));
    let r = passive_clock(
        app,
        &other.token,
        ch["nonce"].as_str().unwrap(),
        colors.len(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["reason"], "spoof");
    let rows = attempts_of(app, other.employee_id).await;
    assert_eq!(rows.last().unwrap().1, "passive_failed");
    // the response follows the negative of the colours: failed (a challenge whose random colours do not
    // happen to correlate with it at lag 1 either)
    let (ch, colors) = loop {
        let ch = challenge_for(app, &other.token).await;
        let colors = colors_of(&ch);
        let means: Vec<[f64; 3]> = colors
            .iter()
            .map(|c| {
                [
                    NEUTRAL_MEAN[0] - 0.5 * c[0],
                    NEUTRAL_MEAN[1] - 0.5 * c[1],
                    NEUTRAL_MEAN[2] - 0.5 * c[2],
                ]
            })
            .collect();
        let j = granary_services::face::flash::judge(NEUTRAL_MEAN, &means, &colors, 1.5, 0.6);
        if j.outcome == granary_services::face::flash::Outcome::Fail {
            break (ch, colors);
        }
    };
    PASSIVE_QUEUE
        .lock()
        .unwrap()
        .push_back(passive_answer(&theirs, &colors, -0.5, 0.95));
    let r = passive_clock(
        app,
        &other.token,
        ch["nonce"].as_str().unwrap(),
        colors.len(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["reason"], "flash_failed");
    // no face in a flash frame: not sure
    let ch = challenge_for(app, &other.token).await;
    let colors = colors_of(&ch);
    let mut answer = passive_answer(&theirs, &colors, 0.5, 0.95);
    answer["flashes"][2]["face"] = json!(false);
    answer["flashes"][2]["region_mean"] = Value::Null;
    PASSIVE_QUEUE.lock().unwrap().push_back(answer);
    let r = passive_clock(
        app,
        &other.token,
        ch["nonce"].as_str().unwrap(),
        colors.len(),
    )
    .await;
    assert_eq!(r.json()["reason"], "no_face", "{}", r.text());
    assert_eq!(
        attempts_of(app, other.employee_id).await.last().unwrap().1,
        "passive_inconclusive"
    );
    let mut db = app.db().await;
    sqlx::query("DELETE FROM face_check_attempts WHERE employee_id = ANY($1)")
        .bind(vec![me.employee_id, other.employee_id])
        .execute(&mut db)
        .await
        .unwrap();
    clear_limits(app).await;
}

#[tokio::test]
async fn test_passive_off_keeps_the_active_flow() {
    let app = app();
    let _g = app.serial().await;
    QUEUE.lock().unwrap().clear();
    let run = seed();
    office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let me = make_person(app, &[]).await;
    let mine = person(run + 901);
    enroll_with(app, &hr, &me, &mine, 2).await;
    let set = |v: &'static str| async move {
        let mut db = app.db().await;
        sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'hr.face.passive.enabled'")
            .bind(v)
            .execute(&mut db)
            .await
            .unwrap();
    };
    set("false").await;
    let ch = challenge_for(app, &me.token).await;
    assert_eq!(ch["passive"]["enabled"], json!(false));
    assert_eq!(ch["passive"]["colors"], json!([]));
    // a passive post on a nonce without colours is refused
    let r = passive_clock(app, &me.token, ch["nonce"].as_str().unwrap(), 5).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("colour"));
    // the old three-frame flow, untouched: no details.passive
    let ok = |a: &str| action_frames(a, &mine);
    let (r, _) = personal_clock(app, &me.token, Some(&ok), Clock::default()).await;
    set("true").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (_, score, _, details) = latest_event(app, me.employee_id).await;
    assert_eq!(score, None);
    assert!(details.get("passive").is_none(), "{details}");

    // Settings refuse values out of range and a settle time not shorter than the flash
    let sa = app.superadmin().await;
    for (key, value) in [
        ("hr.face.passive.flash_ms", "200"),
        ("hr.face.passive.flash_ms", "150"),
        ("hr.face.passive.flash_count", "7"),
        ("hr.face.passive.settle_ms", "400"),
        ("hr.face.passive.palette", "[[1,2,3]]"),
        ("hr.face.passive.flash_min_corr", "2"),
        ("hr.face.passive.enabled", "maybe"),
    ] {
        let r = app
            .patch(
                &sa.token,
                &format!("/api/v1/system/settings/{key}"),
                json!({"value": value}),
            )
            .await;
        assert_eq!(r.status, 400, "{key}={value}: {}", r.text());
        assert!(r.text().contains(key), "{}", r.text());
    }
    let r = app
        .patch(
            &sa.token,
            "/api/v1/system/settings/hr.face.passive.flash_count",
            json!({"value": "5"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    clear_limits(app).await;
}

#[tokio::test]
async fn test_kiosk_passive_settings_and_reported_metrics() {
    let app = app();
    let _g = app.serial().await;
    let run = seed();
    let (office_id, _) = office(app).await;
    let hr = make_person(app, HR_PERMS).await;
    let people = [
        make_person(app, &[]).await,
        make_person(app, &[]).await,
        make_person(app, &[]).await,
    ];
    let faces: Vec<Vec<f64>> = (0..3).map(|i| person(run + 951 + i)).collect();
    for (p, f) in people.iter().zip(&faces) {
        enroll_with(app, &hr, p, f, 2).await;
    }
    let (_, session) = kiosk(app, &hr, office_id).await;
    clear_limits(app).await;
    let r = app
        .post(
            &hr.token,
            "/api/v1/hr/kiosks",
            json!({"name": support::u("Passive tablet"), "office_location_id": office_id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    clear_limits(app).await;
    let s = app
        .send(
            app.client()
                .post(app.url("/api/v1/hr/kiosk/session"))
                .header("X-Kiosk-Token", r.json()["token"].as_str().unwrap()),
        )
        .await;
    assert_eq!(s.status, 200, "{}", s.text());
    assert_eq!(
        s.json()["passive"],
        json!({"enabled": true, "flash_count": 5, "flash_ms": 400, "settle_ms": 340,
               "palette": [[255,120,120],[120,255,140],[120,150,255],[255,230,120],[220,130,255],[120,240,240]],
               "flash_min_delta": 1.5, "flash_min_corr": 0.6, "spoof_min_real": 0.7})
    );
    // the anti-spoof models are on the kiosk proxy's allowlist (the mock engine has none: 404 from it),
    // not on the phones'
    let staff = make_person(app, &[]).await;
    assert_eq!(
        app.get(
            &staff.token,
            "/api/v1/hr/attendance/engine/minifasnet_v2_2.7.onnx"
        )
        .await
        .status,
        404
    );

    let clock = |emp: Uuid, desc: Vec<f64>, liveness: Value| {
        let body = json!({"event_type": "clock_in", "employee_id": emp.to_string(), "descriptor": desc,
                          "distance": 0.2, "action": "turn_left", "liveness": liveness,
                          "captured_at": chrono::Utc::now().to_rfc3339()});
        let s = session.clone();
        async move {
            clear_limits(app).await;
            app.post(&s, "/api/v1/hr/kiosk/clock", body).await
        }
    };
    // passive on the tablet: the random action it sent is not what was checked
    let r = clock(
        people[0].employee_id,
        same(&faces[0], 9),
        json!({"passed": true, "method": "passive",
               "metrics": {"passive": {"corr": 0.91, "delta": 7.5, "spoof_real": 0.88, "lag": 1}}}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (_, score, challenge, details) = latest_event(app, people[0].employee_id).await;
    assert_eq!(score, Some(0.88));
    assert_eq!(challenge.as_deref(), Some("passive"));
    assert_eq!(
        details["passive"],
        json!({"method": "passive", "corr": 0.91, "delta": 7.5, "spoof_real": 0.88, "lag": 1})
    );
    // active after the tablet's passive check was not sure
    let r = clock(
        people[1].employee_id,
        same(&faces[1], 9),
        json!({"passed": true, "method": "active",
               "metrics": {"ear": [0.3, 0.1, 0.3],
                           "passive": {"corr": 0.1, "delta": 1.2, "spoof_real": 0.8, "fallback_reason": "flash_inconclusive"}}}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (_, score, challenge, details) = latest_event(app, people[1].employee_id).await;
    assert_eq!(score, Some(0.8));
    assert_eq!(challenge.as_deref(), Some("turn_left"));
    assert_eq!(details["passive"]["method"], "active");
    assert_eq!(details["passive"]["fallback_reason"], "flash_inconclusive");
    // no face during the flash: the numbers are null
    let r = clock(
        people[2].employee_id,
        same(&faces[2], 9),
        json!({"passed": true, "method": "active",
               "metrics": {"passive": {"corr": null, "delta": null, "spoof_real": null, "fallback_reason": "no_face"}}}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (_, score, _, details) = latest_event(app, people[2].employee_id).await;
    assert_eq!(score, None);
    assert_eq!(
        details["passive"],
        json!({"method": "active", "fallback_reason": "no_face"})
    );
    clear_limits(app).await;
}
