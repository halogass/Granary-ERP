//! Port of the service-level parts of backend/tests/test_face_attendance.py (attendance rules in company
//! time, the challenge store and rate limiter) and of test_audit_gaps_b.py
//! `test_attendance_follows_the_timezone_setting`, plus database tests for matching.gallery/search/
//! max_distance and attendance.notify_review.
//!
//! The HTTP tests of test_face_attendance.py drive routers/face_attendance.py, which is not part of the
//! services crate: they are listed at the bottom as ignored placeholders for the API port.
//!
//! One database for the file (`granary_svc_face`); every test works inside its own transaction and rolls
//! back, as the Python tests do. Redis: db 13 of the test Redis.

mod support;

use std::sync::OnceLock;

use chrono::{NaiveDate, TimeZone, Utc};
use granary_services::face::{attendance, challenge, matching};
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

static DB_URL: OnceLock<String> = OnceLock::new();

fn db_url() -> String {
    DB_URL
        .get_or_init(|| {
            std::env::set_var("REDIS_URL", "redis://127.0.0.1:56380/13");
            std::thread::spawn(|| {
                tokio::runtime::Runtime::new().unwrap().block_on(async {
                    let pool = support::fresh_db("face").await;
                    pool.close().await;
                    std::env::var("DATABASE_URL").unwrap()
                })
            })
            .join()
            .unwrap()
        })
        .clone()
}

async fn conn() -> PgConnection {
    PgConnection::connect(&db_url()).await.unwrap()
}

async fn set_setting(db: &mut PgConnection, key: &str, value: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap()
        .rows_affected();
    if n == 0 {
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, value_type, category) VALUES ($1, $2, $3, 'string', 'system')",
        )
        .bind(Uuid::new_v4())
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap();
    }
}

async fn make_employee(db: &mut PgConnection, first: &str, last: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO employees (id, first_name, last_name, email, is_deleted) VALUES ($1, $2, $3, $4, false)")
        .bind(id)
        .bind(first)
        .bind(last)
        .bind(format!("{id}@pytest.local"))
        .execute(&mut *db)
        .await
        .unwrap();
    id
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

fn conflict_code(e: attendance::RecordError) -> &'static str {
    match e {
        attendance::RecordError::Conflict(c) => c.code,
        attendance::RecordError::Api(a) => panic!("unexpected {a}"),
    }
}

// ─── Attendance rules (company time) ──────────────────────────────────────────

#[tokio::test]
async fn test_attendance_uses_company_time_and_refuses_impossible_sequences() {
    let mut c = conn().await;
    let mut tx = c.begin().await.unwrap();
    let db = &mut *tx;
    // Python monkeypatches _work_start to "08:00"; the setting it reads is set instead
    set_setting(db, "hr.work_start_time", "08:00").await;
    let emp = make_employee(db, "Pytest", Some("Staff")).await;

    // 23:30 UTC on the 20th = 08:30 JST on the 21st -> late by 30 minutes
    let at = utc(2026, 9, 20, 23, 30);
    let result = attendance::record(db, emp, "clock_in", at, None)
        .await
        .unwrap();
    assert!(result.time == "08:30" && result.late);
    assert_eq!(
        result.attendance.date,
        NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
    );
    assert_eq!(result.attendance.late_minutes, Some(30));
    assert_eq!(result.message, "Clocked in at 08:30 (30 min late)");
    assert_eq!(result.attendance.status.as_deref(), Some("late"));
    let dup = attendance::record(db, emp, "clock_in", at + chrono::Duration::hours(1), None)
        .await
        .unwrap_err();
    assert_eq!(conflict_code(dup), "already_clocked_in");

    let out = attendance::record(db, emp, "clock_out", utc(2026, 9, 21, 8, 0), None)
        .await
        .unwrap();
    assert_eq!(out.time, "17:00");
    assert_eq!(out.attendance.work_hours, Some(Decimal::new(850, 2)));
    let again = attendance::record(db, emp, "clock_out", utc(2026, 9, 21, 9, 15), None)
        .await
        .unwrap();
    assert!(again.message.contains("updated from 17:00 to 18:15"));

    // 11:00 UTC = 20:00 JST: clocking in then is on the 22nd
    let night = attendance::record(db, emp, "clock_in", utc(2026, 9, 22, 11, 0), None)
        .await
        .unwrap();
    assert_eq!(
        night.attendance.date,
        NaiveDate::from_ymd_opt(2026, 9, 22).unwrap()
    );
    // an overnight shift closes on the next morning (05:00 JST on the 23rd)
    let close = attendance::record(db, emp, "clock_out", utc(2026, 9, 22, 20, 0), None)
        .await
        .unwrap();
    assert_eq!(
        close.attendance.date,
        NaiveDate::from_ymd_opt(2026, 9, 22).unwrap()
    );
    assert_eq!(close.attendance.work_hours, Some(Decimal::new(900, 2)));

    let none = attendance::record(db, emp, "clock_out", utc(2026, 9, 25, 8, 0), None)
        .await
        .unwrap_err();
    assert_eq!(conflict_code(none), "no_clock_in");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn test_attendance_follows_the_timezone_setting() {
    let mut c = conn().await;
    let mut tx = c.begin().await.unwrap();
    let db = &mut *tx;
    set_setting(db, "hr.work_start_time", "08:00").await;
    let emp = make_employee(db, "Pytest", None).await;
    set_setting(db, "timezone", "Europe/London").await;
    // 07:30 UTC on the 21st is 08:30 in London (16:30 in Tokyo, the test company's zone)
    let result = attendance::record(db, emp, "clock_in", utc(2026, 9, 21, 7, 30), None)
        .await
        .unwrap();
    assert!(result.time == "08:30" && result.late);
    assert_eq!(
        result.attendance.date,
        NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
    );
    assert_eq!(result.attendance.late_minutes, Some(30));
    tx.rollback().await.unwrap();

    let at = utc(2026, 9, 21, 7, 30);
    assert_eq!(
        granary_compat::pyfmt::strftime(
            &attendance::local_py(chrono_tz::Europe::London, Some(at)),
            "%Y-%m-%d %H:%M %Z"
        ),
        "2026-09-21 08:30 BST"
    );
}

#[tokio::test]
async fn clock_in_without_a_parseable_work_start_is_on_time_and_conflicts_are_409() {
    let mut c = conn().await;
    let mut tx = c.begin().await.unwrap();
    let db = &mut *tx;
    set_setting(db, "hr.work_start_time", "8 o'clock").await;
    let emp = make_employee(db, "Pytest", None).await;
    let r = attendance::record(db, emp, "clock_in", utc(2026, 9, 21, 5, 0), None)
        .await
        .unwrap();
    assert!(
        !r.late
            && r.message == "Clocked in at 14:00"
            && r.attendance.status.as_deref() == Some("present")
    );
    let err: granary_core::errors::ApiError =
        attendance::record(db, emp, "clock_in", utc(2026, 9, 21, 6, 0), None)
            .await
            .unwrap_err()
            .into();
    assert_eq!(err.status().as_u16(), 409);
    assert_eq!(
        serde_json::to_string(&err.body()).unwrap(),
        r#"{"detail":{"code":"already_clocked_in","message":"Already clocked in today at 14:00"}}"#
    );
    assert_eq!(attendance::employee_name(db, emp).await.unwrap(), "Pytest");
    assert_eq!(
        attendance::employee_name(db, Uuid::new_v4()).await.unwrap(),
        "Employee"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn notify_review_reaches_attendance_managers_and_superadmins() {
    let mut c = conn().await;
    let mut tx = c.begin().await.unwrap();
    let db = &mut *tx;
    let mut users = vec![];
    for active in [true, false] {
        let uid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
             VALUES ($1, $2, 'HR', 'x', $3, false, false, 'internal')",
        )
        .bind(uid)
        .bind(format!("{uid}@pytest.local"))
        .bind(active)
        .execute(&mut *db)
        .await
        .unwrap();
        let role: Uuid = sqlx::query_scalar(
            "SELECT rpm.role_id FROM role_permissions rpm JOIN permissions p ON p.id = rpm.permission_id \
             WHERE p.code = 'hr:manage_attendance' LIMIT 1",
        )
        .fetch_one(&mut *db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(uid)
            .bind(role)
            .execute(&mut *db)
            .await
            .unwrap();
        users.push(uid.to_string());
    }
    let ids = attendance::hr_reviewer_ids(db).await.unwrap();
    assert!(ids.contains(&users[0]) && !ids.contains(&users[1]));
    let mut dedup = ids.clone();
    dedup.dedup();
    assert_eq!(dedup.len(), ids.len());

    let event_id = Uuid::new_v4();
    attendance::notify_review_for(
        db,
        event_id,
        "clock_out",
        utc(2026, 9, 21, 1, 5),
        "weak match",
    )
    .await
    .unwrap();
    let rows: Vec<(String, String, Option<String>, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT user_id::text, title, body, link, entity_type, priority FROM notifications WHERE entity_id = $1",
    )
    .bind(event_id.to_string())
    .fetch_all(&mut *db)
    .await
    .unwrap();
    assert_eq!(rows.len(), ids.len());
    let mine = rows.iter().find(|r| r.0 == users[0]).unwrap();
    assert_eq!(mine.1, "Clock-out needs review (21 Sep 10:05)");
    assert_eq!(mine.2.as_deref(), Some("weak match"));
    assert_eq!(mine.3.as_deref(), Some("/hr/attendance?tab=review"));
    assert_eq!(mine.4.as_deref(), Some("clock_event"));
    assert_eq!(mine.5.as_deref(), Some("normal"));
    tx.rollback().await.unwrap();
}

// ─── Matching against the database ────────────────────────────────────────────

#[tokio::test]
async fn gallery_search_and_max_distance() {
    let mut c = conn().await;
    let mut tx = c.begin().await.unwrap();
    let db = &mut *tx;
    // start from an empty gallery inside this transaction
    sqlx::query("UPDATE face_enrollments SET is_active = false")
        .execute(&mut *db)
        .await
        .unwrap();
    let (ann, ben, gone) = (
        make_employee(db, "Ann", None).await,
        make_employee(db, "Ben", None).await,
        make_employee(db, "Gone", None).await,
    );
    let vec_of = |seed: f64| -> Vec<f64> {
        (0..128)
            .map(|i| ((i as f64 + seed) * 0.37).sin() * 0.09)
            .collect()
    };
    for (emp, active, seeds) in [
        (ann, true, vec![1.0, 1.01]),
        (ben, true, vec![50.0]),
        (gone, false, vec![1.0]),
    ] {
        sqlx::query("INSERT INTO face_enrollments (id, employee_id, is_active, image_count) VALUES ($1, $2, $3, 1)")
            .bind(Uuid::new_v4())
            .bind(emp)
            .bind(active)
            .execute(&mut *db)
            .await
            .unwrap();
        for s in seeds {
            sqlx::query("INSERT INTO face_descriptors (id, employee_id, descriptor, model) VALUES ($1, $2, $3, 'faceapi-1.7')")
                .bind(Uuid::new_v4())
                .bind(emp)
                .bind(vec_of(s))
                .execute(&mut *db)
                .await
                .unwrap();
        }
    }
    let (ids, matrix) = matching::gallery(db, None).await.unwrap();
    assert_eq!(ids.len(), 3);
    assert!(!ids.contains(&gone.to_string()));
    assert!(matrix.iter().all(|r| r.len() == 128));
    let (only, _) = matching::gallery(db, Some(&[ben])).await.unwrap();
    assert_eq!(only, vec![ben.to_string()]);

    let q = vec_of(1.005);
    let found = matching::search(db, &[&q]).await.unwrap();
    assert_eq!(found.employee_id, Some(ann.to_string()));
    assert_eq!(found.runner_up_id, Some(ben.to_string()));
    assert_eq!(found, matching::search_in(&ids, &matrix, &[&q]));

    for (value, want) in [
        ("0.50", 0.5),
        ("0.1", 0.30),
        ("0.9", 0.65),
        ("", 0.5),
        ("abc", 0.5),
        (" 0.45 ", 0.45),
    ] {
        set_setting(db, "hr.face_match_distance", value).await;
        assert_eq!(matching::max_distance(db).await.unwrap(), want, "{value:?}");
    }
    sqlx::query("DELETE FROM system_settings WHERE key = 'hr.face_match_distance'")
        .execute(&mut *db)
        .await
        .unwrap();
    assert_eq!(matching::max_distance(db).await.unwrap(), 0.5);
    tx.rollback().await.unwrap();
}

// ─── Challenges and the rate limiter (Redis) ──────────────────────────────────

#[tokio::test]
async fn new_challenges_ask_only_for_head_turns() {
    db_url(); // env (REDIS_URL) first
    let user = Uuid::new_v4().to_string();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..40 {
        let issued = challenge::issue("clock_in", &user).await.unwrap();
        assert_ne!(issued.action, "blink");
        seen.insert(issued.action);
    }
    assert_eq!(seen.len(), 2, "{seen:?}");
}

#[tokio::test]
async fn challenge_is_single_use_and_bound_to_the_request() {
    db_url(); // env (REDIS_URL) first
    let user = Uuid::new_v4().to_string();
    let issued = challenge::issue("clock_in", &user).await.unwrap();
    assert_eq!(issued.nonce.len(), 32);
    assert!(granary_services::face::liveness::ISSUED_ACTIONS.contains(&issued.action.as_str()));
    assert_eq!(issued.expires_in, 45);
    assert_eq!(
        serde_json::to_value(&issued)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["nonce", "action", "expires_in"]
    );
    assert_eq!(
        challenge::consume(&issued.nonce, "clock_in", &user)
            .await
            .unwrap(),
        issued.action
    );
    let again = challenge::consume(&issued.nonce, "clock_in", &user)
        .await
        .unwrap_err();
    assert_eq!(again.body()["detail"], "The check expired: start again");

    let other = challenge::issue("clock_in", &user).await.unwrap();
    let wrong = challenge::consume(&other.nonce, "clock_out", &user)
        .await
        .unwrap_err();
    assert_eq!(wrong.status().as_u16(), 400);
    assert_eq!(
        wrong.body()["detail"],
        "The check does not belong to this request: start again"
    );
    // spent even though it failed
    assert!(challenge::consume(&other.nonce, "clock_in", &user)
        .await
        .is_err());
    for bad in ["", &"x".repeat(65)] {
        assert_eq!(
            challenge::consume(bad, "clock_in", &user)
                .await
                .unwrap_err()
                .status()
                .as_u16(),
            400
        );
    }
}

#[tokio::test]
async fn test_rate_limit_answers_429() {
    db_url();
    let key = format!("challenge:user:{}", Uuid::new_v4());
    for _ in 0..10 {
        challenge::rate_limit(&key, 10, 60).await.unwrap();
    }
    let r = challenge::rate_limit(&key, 10, 60).await.unwrap_err();
    assert_eq!(r.status().as_u16(), 429);
    assert_eq!(
        r.body()["detail"],
        "Too many attempts: wait a minute and try again"
    );
}

// ─── HTTP tests of routers/face_attendance.py (API layer) ─────────────────────

#[tokio::test]
#[ignore = "covered by crates/api/tests/face_attendance_routes.rs::test_enrollment_stores_descriptors_and_refuses_bad_photos (wave 4)"]
async fn test_enrollment_stores_descriptors_and_refuses_bad_photos() {}

#[tokio::test]
#[ignore = "covered by crates/api/tests/face_attendance_routes.rs::test_personal_clock_in_needs_login_liveness_gps_and_your_own_face (wave 4)"]
async fn test_personal_clock_in_needs_login_liveness_gps_and_your_own_face() {}

/// Known flaky in Python: `drifted = mine + person(RUN + 302) * 0.55` puts the distance only around 0.55
/// (random per run), so some runs land outside the 0.50-0.60 review band and the test fails.
#[tokio::test]
#[ignore = "covered by crates/api/tests/face_attendance_routes.rs::test_weak_personal_match_goes_to_hr_review (wave 4)"]
async fn test_weak_personal_match_goes_to_hr_review() {}

#[tokio::test]
#[ignore = "covered by crates/api/tests/face_attendance_routes.rs::test_engine_and_gallery_only_after_kiosk_authentication (wave 4)"]
async fn test_engine_and_gallery_only_after_kiosk_authentication() {}

#[tokio::test]
#[ignore = "covered by crates/api/tests/face_attendance_routes.rs::test_kiosk_clock_is_revalidated_by_the_servers_own_search (wave 4)"]
async fn test_kiosk_clock_is_revalidated_by_the_servers_own_search() {}
