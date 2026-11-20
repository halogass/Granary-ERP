//! Port of the services test marked `#[ignore = "wave 4: routers.knowledge._open_revision /
//! _create_revision (the SOP revision)"]` in crates/services/tests/nary_cadence_deals_db.rs
//! (tests/test_nary_p3_deals.py::test_sop_revisions_are_drafts_in_the_existing_signing_flow): Nary's SOP
//! revision drafts go through the knowledge router's revision helpers.

mod support;

#[path = "../../services/tests/nary_n10_common/mod.rs"]
#[allow(unused_imports)]
mod nary_n10_common;

use granary_db::rows::knowledge::KBArticle;
use granary_services::ai::registry::seam::{Answer, Brain};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

fn sop_answer() -> Value {
    json!({"title": "Lead Qualification Process", "summary": "Nary drafts, Owner approves",
           "content": "## Purpose\nNary qualifies.\n---\n## \u{1f4cb} Japanese Version\nNary.",
           "version_notes": "BDR is Nary.", "sop_trigger": "New lead", "sop_owner_role": "Nary (BDR)"})
}

#[tokio::test]
async fn test_sop_revisions_are_drafts_in_the_existing_signing_flow() {
    let app = support::app("kb_sop_rev");
    let pool = sqlx::PgPool::connect(&app.db_url).await.unwrap();
    granary_services::nary::set_session_pool(pool.clone());
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = nary_n10_common::owner(db).await;
    let user = nary_n10_common::auth_user(db, me).await;
    let code = "SOP-001";
    // the seed has no briefs: the test names what to change in both SOPs
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.sop_revisions.briefs'")
        .bind(
            json!({code: "Lead qualification is done by Nary.", "SOP-002": "A new SOP for the work queue."})
                .to_string(),
        )
        .execute(&mut *db)
        .await
        .unwrap();
    // and which SOPs are related reference and which code is the new one
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.sop_revisions.codes'")
        .bind(json!({"related": [code], "new": "SOP-002"}).to_string())
        .execute(&mut *db)
        .await
        .unwrap();
    let live: Option<KBArticle> = sqlx::query_as("SELECT * FROM kb_articles WHERE sop_code = $1")
        .bind(code)
        .fetch_optional(&mut *db)
        .await
        .unwrap();
    let made = live.is_none();
    let live_id = match live {
        Some(l) => {
            sqlx::query("UPDATE kb_articles SET status = 'published' WHERE id = $1")
                .bind(l.id)
                .execute(&mut *db)
                .await
                .unwrap();
            l.id
        }
        None => {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO kb_articles (id, title, slug, content, article_type, sop_code, status, version, is_private, is_deleted) \
                 VALUES ($1, 'Lead Qualification Process', $2, 'Old text', 'sop', $3, 'published', '1.0', false, false)",
            )
            .bind(id)
            .bind(support::u("pytest-sop"))
            .bind(code)
            .execute(&mut *db)
            .await
            .unwrap();
            id
        }
    };
    brain.set("nary.sop_revision", Answer::json(sop_answer()));
    let out = granary_services::nary::sop_revisions::draft_many(
        db,
        &[code.to_string(), "SOP-002".to_string()],
        &user,
    )
    .await
    .unwrap();
    let codes: Vec<&str> = out["drafted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, vec![code, "SOP-002"]);
    assert_eq!(out["failed"], json!([]));
    let rev_id = Uuid::parse_str(out["drafted"][0]["id"].as_str().unwrap()).unwrap();
    let rev: KBArticle = sqlx::query_as("SELECT * FROM kb_articles WHERE id = $1")
        .bind(rev_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(
        rev.previous_version_id == Some(live_id)
            && rev.status.as_deref() == Some("draft")
            && rev.sop_code.is_none()
    );
    // live until signed and approved
    let live_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM kb_articles WHERE id = $1")
            .bind(live_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(live_status.as_deref(), Some("published"));
    let new_id = Uuid::parse_str(out["drafted"][1]["id"].as_str().unwrap()).unwrap();
    let new: KBArticle = sqlx::query_as("SELECT * FROM kb_articles WHERE id = $1")
        .bind(new_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(new.sop_code.as_deref() == Some("SOP-002") && new.status.as_deref() == Some("draft"));
    let out2 = granary_services::nary::sop_revisions::draft_many(db, &[code.to_string()], &user)
        .await
        .unwrap();
    assert_eq!(out2["failed"][0]["code"], code); // one open revision at a time
    sqlx::query("DELETE FROM kb_articles WHERE id = ANY($1)")
        .bind(vec![rev_id, new_id])
        .execute(&mut *db)
        .await
        .unwrap();
    if made {
        sqlx::query("DELETE FROM kb_articles WHERE id = $1")
            .bind(live_id)
            .execute(&mut *db)
            .await
            .unwrap();
    }
}

/// `_bump_major`: the revision's version.
#[test]
fn test_bump_major() {
    use granary_services::later::nary_sop_revisions::_bump_major;
    assert_eq!(_bump_major(Some("1.3")), "2.0");
    assert_eq!(_bump_major(Some(" 2 ")), "3.0");
    assert_eq!(_bump_major(Some("v1.0")), "v1.0.1");
    assert_eq!(_bump_major(None), "None.1");
}
