//! Port of the bulk-run half of backend/tests/test_crm_bulk_ai.py (the duplicate-scan half belongs to
//! crm/dedupe). The Python tests replace `bulk_ai.enrich_organization`; here `bulk_ai::seam::with_enrich`.
mod support;

use std::sync::{Arc, Mutex};

use chrono::Utc;
use granary_core::auth::AuthUser;
use granary_db::rows::crm::Organization;
use granary_services::ai::enrichment::{EnrichError, EnrichmentUnavailable};
use granary_services::crm::bulk_ai::{self, seam, BulkFail};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

async fn user(db: &mut PgConnection, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Pytest person', 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{id}@pytest.test"))
    .execute(&mut *db)
    .await
    .unwrap();
    AuthUser {
        id,
        email: format!("{id}@pytest.test"),
        name: "Pytest person".into(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: true,
        is_superadmin: false,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        perms: perms.iter().map(|s| s.to_string()).collect(),
    }
}

async fn org(db: &mut PgConnection, name: &str, assigned: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, assigned_to_id, is_deleted, custom) VALUES ($1, $2, $3, false, '{}')")
        .bind(id)
        .bind(name)
        .bind(assigned)
        .execute(&mut *db)
        .await
        .unwrap();
    id
}

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

fn ok_enrich() -> Value {
    json!({"status": "ok", "written": {}, "suggested": {}, "sources": []})
}

#[tokio::test]
async fn test_a_bulk_enrichment_reports_every_record() {
    let pool = support::fresh_db("bulk_every").await;
    let mut db = pool.acquire().await.unwrap();
    let u = user(&mut db, &["crm:view_all", "crm:edit_contact"]).await;
    let mut ids = vec![];
    for _ in 0..3 {
        ids.push(org(&mut db, &format!("Bulk {}", tag()), None).await);
    }
    let seen = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    let f: seam::EnrichFn = Arc::new(move |o: &mut Organization, _| {
        s2.lock().unwrap().push(o.id);
        o.city = Some("Nagoya".into());
        Ok(json!({"status": "ok", "written": {"city": "Nagoya"}, "suggested": {}, "sources": []}))
    });
    let rids: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    let summary = seam::with_enrich(
        f,
        bulk_ai::run_bulk(&mut db, &u, "organization", &rids, None, "enrich"),
    )
    .await
    .unwrap();
    assert_eq!(summary["succeeded"], json!(3));
    assert_eq!(summary["failed"], json!([]));
    let mut got = seen.lock().unwrap().clone();
    got.sort();
    ids.sort();
    assert_eq!(got, ids);
    assert_eq!(summary["label"], json!("Enriched from the web"));
}

#[tokio::test]
async fn test_a_spent_budget_stops_the_run_instead_of_failing_every_row() {
    let pool = support::fresh_db("bulk_budget").await;
    let mut db = pool.acquire().await.unwrap();
    let u = user(&mut db, &["crm:view_all", "crm:edit_contact"]).await;
    let mut rids = vec![];
    for _ in 0..4 {
        rids.push(
            org(&mut db, &format!("Bulk {}", tag()), None)
                .await
                .to_string(),
        );
    }
    let calls = Arc::new(Mutex::new(0));
    let c2 = calls.clone();
    let f: seam::EnrichFn = Arc::new(move |_o: &mut Organization, _| {
        *c2.lock().unwrap() += 1;
        Err(EnrichError::Unavailable(EnrichmentUnavailable(
            "The monthly AI budget ($5.00) is used up.".into(),
        )))
    });
    let summary = seam::with_enrich(
        f,
        bulk_ai::run_bulk(&mut db, &u, "organization", &rids, None, "enrich"),
    )
    .await
    .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "the second record must not be attempted"
    );
    assert!(summary["stopped_early"]
        .as_str()
        .unwrap()
        .contains("budget"));
    assert_eq!(summary["failed"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_a_record_assigned_to_someone_else_is_skipped_not_read() {
    let pool = support::fresh_db("bulk_owner").await;
    let mut db = pool.acquire().await.unwrap();
    let u = user(&mut db, &["crm:view_own", "crm:edit_contact"]).await;
    let colleague = user(&mut db, &["crm:view_own"]).await;
    let mine = org(&mut db, &format!("Mine {}", tag()), Some(u.id)).await;
    let theirs = org(&mut db, &format!("Theirs {}", tag()), Some(colleague.id)).await;
    let touched = Arc::new(Mutex::new(vec![]));
    let t2 = touched.clone();
    let f: seam::EnrichFn = Arc::new(move |o: &mut Organization, _| {
        t2.lock().unwrap().push(o.id);
        Ok(ok_enrich())
    });
    let rids = vec![mine.to_string(), theirs.to_string()];
    let summary = seam::with_enrich(
        f,
        bulk_ai::run_bulk(&mut db, &u, "organization", &rids, None, "enrich"),
    )
    .await
    .unwrap();
    assert_eq!(*touched.lock().unwrap(), vec![mine]);
    assert_eq!(summary["failed"][0]["record_id"], json!(theirs.to_string()));
}

#[tokio::test]
async fn test_one_bad_record_does_not_end_the_run() {
    let pool = support::fresh_db("bulk_one_bad").await;
    let mut db = pool.acquire().await.unwrap();
    let u = user(&mut db, &["crm:view_all", "crm:edit_contact"]).await;
    let good = org(&mut db, &format!("Good {}", tag()), None).await;
    let bad = org(&mut db, &format!("Bad {}", tag()), None).await;
    // Python raised AIAttributeError from the stand-in; a known failure without a stop word.
    let f: seam::EnrichFn = Arc::new(move |o: &mut Organization, _| {
        if o.id == bad {
            Err(EnrichError::Unavailable(EnrichmentUnavailable(
                "The model returned nothing usable.".into(),
            )))
        } else {
            Ok(ok_enrich())
        }
    });
    let rids = vec![bad.to_string(), good.to_string()];
    let summary = seam::with_enrich(
        f,
        bulk_ai::run_bulk(&mut db, &u, "organization", &rids, None, "enrich"),
    )
    .await
    .unwrap();
    assert_eq!(summary["succeeded"], json!(1));
    assert_eq!(summary["failed"].as_array().unwrap().len(), 1);
    assert_eq!(summary["stopped_early"], Value::Null);
}

#[tokio::test]
async fn test_a_bad_uuid_and_a_missing_record_are_reported() {
    let pool = support::fresh_db("bulk_missing").await;
    let mut db = pool.acquire().await.unwrap();
    let u = user(&mut db, &["crm:view_all"]).await;
    let f: seam::EnrichFn = Arc::new(|_: &mut Organization, _| Ok(ok_enrich()));
    let rids = vec!["nope".to_string(), Uuid::new_v4().to_string()];
    let summary = seam::with_enrich(
        f,
        bulk_ai::run_bulk(&mut db, &u, "organization", &rids, None, "enrich"),
    )
    .await
    .unwrap();
    assert_eq!(
        summary["failed"],
        json!([{"record_id": "nope", "error": "400: Invalid record_id format"},
               {"record_id": rids[1], "error": "No longer exists"}])
    );
}

#[tokio::test]
async fn test_the_requester_is_notified_when_it_finishes() {
    let pool = support::fresh_db("bulk_notify").await;
    let mut db = pool.acquire().await.unwrap();
    let u = user(&mut db, &[]).await;
    let summary = json!({"label": "Filled Tier", "succeeded": 28, "requested": 30,
        "failed": [{"record_id": "x", "error": "gone"}, {"record_id": "x", "error": "gone"}],
        "stopped_early": null});
    bulk_ai::notify_finished(&mut db, u.id, "organization", &summary)
        .await
        .unwrap();
    let (title, body, link): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT title, body, link FROM notifications WHERE user_id = $1")
            .bind(u.id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(title.contains("28 of 30"));
    assert_eq!(title, "Filled Tier for 28 of 30");
    assert_eq!(
        body.as_deref(),
        Some("28 of 30 organizations done. 2 could not be processed.")
    );
    assert_eq!(link.as_deref(), Some("/crm/organizations"));
}

#[tokio::test]
async fn test_a_bad_request_is_refused_before_anything_runs() {
    let pool = support::fresh_db("bulk_refused").await;
    let mut db = pool.acquire().await.unwrap();
    let u = user(&mut db, &["crm:view_all"]).await;
    let fifty_one: Vec<String> = vec!["x".to_string(); 51];
    for (object_type, action, ids, message) in [
        ("contact", "enrich", vec!["x".to_string()], "companies"),
        ("organization", "attribute", vec![], "at least one"),
        ("organization", "enrich", fifty_one, "max"),
    ] {
        match bulk_ai::run_bulk(&mut db, &u, object_type, &ids, None, action).await {
            Err(BulkFail::Bulk(e)) => assert!(e.0.contains(message), "{e}"),
            other => panic!("{other:?}"),
        }
    }
    match bulk_ai::run_bulk(
        &mut db,
        &u,
        "organization",
        &["x".to_string()],
        Some("tier"),
        "attribute",
    )
    .await
    {
        Err(BulkFail::Bulk(e)) => assert_eq!(e.0, "No active AI attribute 'tier' on organization."),
        other => panic!("{other:?}"),
    }
}
