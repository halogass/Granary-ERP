//! Duplicate scan / list / preview / merge / dismiss against a database: the service side of
//! backend/tests/test_crm_ai_api.py (Phase 5), test_audit_medium_w3.py and test_crm_bulk_ai.py.
//! The HTTP wrappers (routers/crm_records.py) are thin: `_require` permission checks, `_merge_access`
//! owner scoping, and DedupeError -> 400; their own checks are ported with the routers.

mod crm_s16_common;

use crm_s16_common::{db, make_person, u};
use granary_services::crm::dedupe::{self, DedupeError};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

const TAG: &str = "s16_dedupe";

async fn org(
    c: &mut PgConnection,
    name: &str,
    email: Option<&str>,
    city: Option<&str>,
    phone: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, email, city, phone, type, tags, revenue_currency, is_deleted, custom) \
         VALUES ($1, $2, $3, $4, $5, '{prospect}', '{}', 'USD', false, '{}')",
    )
    .bind(id)
    .bind(name)
    .bind(email)
    .bind(city)
    .bind(phone)
    .execute(&mut *c)
    .await
    .unwrap();
    id
}

fn invalid(r: Result<Value, DedupeError>) -> String {
    match r {
        Err(DedupeError::Invalid(m)) => m,
        other => panic!("expected a DedupeError, got {other:?}"),
    }
}

fn s(id: Uuid) -> String {
    id.hyphenated().to_string()
}

#[tokio::test]
async fn test_scan_finds_and_merge_resolves_a_duplicate() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let shared = format!("{}@example.com", u("dupe"));
    let a = org(
        &mut c,
        "Duplicate Works Ltd.",
        Some(&shared),
        Some("Osaka"),
        None,
    )
    .await;
    let b = org(&mut c, "Duplicate Works Inc", Some(&shared), None, None).await;

    let scan = dedupe::scan(&mut c, "organization", 70.0, 500)
        .await
        .unwrap();
    assert!(scan["scanned"].as_i64().unwrap() >= 2);
    assert_eq!(scan["from_embeddings"], 0);

    let listed = dedupe::list_candidates(&mut c, Some("organization"), "open", 50)
        .await
        .unwrap();
    let pair = listed
        .iter()
        .find(|p| {
            let ids = [
                p["a"]["id"].as_str().unwrap(),
                p["b"]["id"].as_str().unwrap(),
            ];
            ids.contains(&s(a).as_str()) && ids.contains(&s(b).as_str())
        })
        .expect("the identical-email pair should be suggested");
    assert!(pair["score"].as_f64().unwrap() >= 95.0);

    let preview = dedupe::merge_preview(&mut c, "organization", a, b)
        .await
        .unwrap();
    let city = preview["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["field"] == "city")
        .unwrap();
    assert_eq!(city["result"], "Osaka");
    assert_eq!(city["taken_from"], "keep");

    let cid = Uuid::parse_str(pair["id"].as_str().unwrap()).unwrap();
    let merged = dedupe::merge_records(&mut c, &user, "organization", a, b, Some(cid))
        .await
        .unwrap();
    assert_eq!(merged["ok"], true);

    // The loser is soft-deleted; the survivor remains.
    let deleted: Vec<(Uuid, bool)> =
        sqlx::query_as("SELECT id, is_deleted FROM organizations WHERE id = ANY($1)")
            .bind(vec![a, b])
            .fetch_all(&mut *c)
            .await
            .unwrap();
    assert!(deleted.contains(&(a, false)) && deleted.contains(&(b, true)));
    let (status, into): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, merged_into_id FROM crm_duplicate_candidates WHERE id = $1")
            .bind(cid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!((status.as_str(), into), ("merged", Some(a)));
    // and the audit row, its JSON written as Python's json.dumps writes it
    let (old, changed): (String, Option<Vec<String>>) = sqlx::query_as(
        "SELECT old_values::text, changed_fields FROM audit_logs WHERE action = 'merge' AND entity_id = $1",
    )
    .bind(s(a))
    .fetch_one(&mut *c)
    .await
    .unwrap();
    assert_eq!(
        old,
        format!(
            "{{\"merged_record\": \"{}\", \"label\": \"Duplicate Works Inc\"}}",
            s(b)
        )
    );
    assert_eq!(changed, None);
}

#[tokio::test]
async fn test_merge_fills_blank_fields_from_the_loser() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let shared = format!("{}@example.com", u("fill"));
    let a = org(&mut c, "Blank Ltd", Some(&shared), None, None).await;
    let b = org(
        &mut c,
        "Blank Inc",
        Some(&shared),
        None,
        Some("+818123456789"),
    )
    .await;
    let preview = dedupe::merge_preview(&mut c, "organization", a, b)
        .await
        .unwrap();
    let phone = preview["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["field"] == "phone")
        .unwrap();
    assert_eq!(phone["taken_from"], "merge");
    dedupe::merge_records(&mut c, &user, "organization", a, b, None)
        .await
        .unwrap();
    let phone: Option<String> = sqlx::query_scalar("SELECT phone FROM organizations WHERE id = $1")
        .bind(a)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(phone.as_deref(), Some("+818123456789"));
    let changed: Option<Vec<String>> = sqlx::query_scalar(
        "SELECT changed_fields FROM audit_logs WHERE action = 'merge' AND entity_id = $1",
    )
    .bind(s(a))
    .fetch_one(&mut *c)
    .await
    .unwrap();
    assert_eq!(changed, Some(vec!["phone".to_string()]));
}

#[tokio::test]
async fn test_merge_moves_related_records() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let shared = format!("{}@example.com", u("move"));
    let a = org(&mut c, "Mover Ltd", Some(&shared), None, None).await;
    let b = org(&mut c, "Mover Inc", Some(&shared), None, None).await;
    let deal = Uuid::new_v4();
    sqlx::query("INSERT INTO deals (id, title, stage, organization_id, is_deleted, custom) VALUES ($1, $2, 'new', $3, false, '{}')")
        .bind(deal)
        .bind(u("Moved"))
        .bind(b)
        .execute(&mut *c)
        .await
        .unwrap();
    let preview = dedupe::merge_preview(&mut c, "organization", a, b)
        .await
        .unwrap();
    assert_eq!(
        preview["moves"],
        serde_json::json!([{"what": "deals", "count": 1}])
    );
    let r = dedupe::merge_records(&mut c, &user, "organization", a, b, None)
        .await
        .unwrap();
    assert!(r["moved_rows"].as_i64().unwrap() >= 1);
    let now_org: Option<Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM deals WHERE id = $1")
            .bind(deal)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(now_org, Some(a));
}

#[tokio::test]
async fn test_merge_into_self_is_rejected() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let a = org(&mut c, &u("Self"), None, None, None).await;
    let msg = invalid(dedupe::merge_records(&mut c, &user, "organization", a, a, None).await);
    assert_eq!(msg, "Cannot merge a record into itself.");
    // routers turn it into a 400
    let api: granary_core::errors::ApiError = DedupeError::Invalid(msg).into();
    assert_eq!(api.status().as_u16(), 400);
    assert_eq!(
        invalid(dedupe::merge_preview(&mut c, "invoice", a, a).await),
        "object_type must be one of organization, contact"
    );
}

#[tokio::test]
async fn test_dismiss_removes_a_candidate_from_the_open_list() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let shared = format!("{}@example.com", u("dismiss"));
    org(&mut c, "Ignored Ltd", Some(&shared), None, None).await;
    org(&mut c, "Ignored Inc", Some(&shared), None, None).await;
    dedupe::scan(&mut c, "organization", 70.0, 500)
        .await
        .unwrap();
    let listed = dedupe::list_candidates(&mut c, Some("organization"), "open", 50)
        .await
        .unwrap();
    let target = listed
        .iter()
        .find(|p| {
            p["a"]["label"].as_str().unwrap().contains("Ignored")
                || p["b"]["label"].as_str().unwrap().contains("Ignored")
        })
        .unwrap()
        .clone();
    let tid = Uuid::parse_str(target["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        dedupe::dismiss(&mut c, &user, tid).await.unwrap(),
        serde_json::json!({"ok": true})
    );
    let again = dedupe::list_candidates(&mut c, Some("organization"), "open", 50)
        .await
        .unwrap();
    assert!(again.iter().all(|p| p["id"] != target["id"]));
    assert_eq!(
        invalid(dedupe::dismiss(&mut c, &user, Uuid::new_v4()).await),
        "Candidate not found."
    );
}

/// test_audit_medium_w3.py: a second merge of the same pair is refused (the service half of
/// test_ai_preview_and_merge_respect_owner_and_deletion; its 403/404 checks are the routers').
#[tokio::test]
async fn test_ai_preview_and_merge_respect_owner_and_deletion() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let keep = org(&mut c, &u("Pytest W3 Keep"), None, None, None).await;
    let merge = org(&mut c, &u("Pytest W3 Merge"), None, None, None).await;
    dedupe::merge_records(&mut c, &user, "organization", keep, merge, None)
        .await
        .unwrap();
    let msg =
        invalid(dedupe::merge_records(&mut c, &user, "organization", keep, merge, None).await);
    assert!(msg.contains("already merged"), "{msg}");
}

/// test_audit_medium_w3.py
#[tokio::test]
async fn test_merge_service_refuses_an_already_merged_record() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let keep = org(&mut c, &u("Pytest W3 K"), None, None, None).await;
    let gone = org(&mut c, &u("Pytest W3 G"), None, None, None).await;
    sqlx::query("UPDATE organizations SET is_deleted = true WHERE id = $1")
        .bind(gone)
        .execute(&mut *c)
        .await
        .unwrap();
    invalid(dedupe::merge_preview(&mut c, "organization", keep, gone).await);
}

/// test_crm_bulk_ai.py: with no embedding provider the deterministic pass still runs.
#[tokio::test]
async fn test_the_scan_still_works_when_ai_is_unavailable() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let a = org(
        &mut c,
        &format!("Duplicate Ltd {}", u("x")),
        Some("dup@pytest.test"),
        None,
        None,
    )
    .await;
    let b = org(
        &mut c,
        &format!("Duplicate Co {}", u("y")),
        Some("dup@pytest.test"),
        None,
        None,
    )
    .await;
    let out = dedupe::scan(&mut c, "organization", 70.0, 500)
        .await
        .unwrap();
    assert_eq!(out["from_embeddings"], 0);
    let rows: Vec<(Value,)> = sqlx::query_as(
        "SELECT reasons FROM crm_duplicate_candidates WHERE record_a_id = ANY($1) AND record_b_id = ANY($1)",
    )
    .bind(vec![a, b])
    .fetch_all(&mut *c)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0]
        .0
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r.as_str().unwrap().contains("email")));
    // a rescan updates, never duplicates
    let again = dedupe::scan(&mut c, "organization", 70.0, 500)
        .await
        .unwrap();
    assert_eq!(again["new_candidates"], 0);
}

/// test_crm_bulk_ai.py `_vector`: 768 dims from the alphabet of the words.
fn vector(text: &str) -> granary_db::rows::Vector {
    let mut v = vec![0f64; 768];
    for word in dedupe::normalize_name(Some(text)).split_whitespace() {
        for ch in word.chars() {
            v[(ch as usize) % 768] += 1.0;
        }
    }
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm = if norm == 0.0 { 1.0 } else { norm };
    granary_db::rows::Vector(v.into_iter().map(|x| (x / norm) as f32).collect())
}

/// test_crm_bulk_ai.py, with the vectors stored up front instead of a stub provider (the provider is
/// wave 3): every live organization already has an up-to-date embedding, so the scan's AI pass runs on
/// pgvector without embedding anything.
#[tokio::test]
async fn test_the_same_company_under_another_name_is_raised() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let a = org(&mut c, "Vandelay Industries Group Ltd", None, None, None).await;
    let b = org(&mut c, "Vandelay Industries Group Inc", None, None, None).await;
    let all: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM organizations WHERE is_deleted = false")
            .fetch_all(&mut *c)
            .await
            .unwrap();
    sqlx::query("DELETE FROM crm_embeddings")
        .execute(&mut *c)
        .await
        .unwrap();
    for (id, name) in all {
        let label = name.trim().to_string();
        if label.is_empty() {
            continue;
        }
        sqlx::query("INSERT INTO crm_embeddings (id, object_type, record_id, label, embedding) VALUES ($1, 'organization', $2, $3, $4)")
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(&label)
            .bind(vector(&label))
            .execute(&mut *c)
            .await
            .unwrap();
    }
    let out = dedupe::scan(&mut c, "organization", 70.0, 500)
        .await
        .unwrap();
    assert!(out["scanned"].as_i64().unwrap() >= 2);
    let rows: Vec<(Value,)> = sqlx::query_as(
        "SELECT reasons FROM crm_duplicate_candidates WHERE record_a_id = ANY($1) AND record_b_id = ANY($1)",
    )
    .bind(vec![a, b])
    .fetch_all(&mut *c)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "one pair, however many passes found it");
    assert!(rows[0]
        .0
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r.as_str().unwrap().contains("AI")));
    sqlx::query("DELETE FROM crm_embeddings")
        .execute(&mut *c)
        .await
        .unwrap();
}

/// The `embeddings` fixture's toy provider: same words (in any order) give the same vector.
struct StubEmbed;

#[async_trait::async_trait]
impl granary_services::ai::base::LLMProvider for StubEmbed {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(
        &self,
        _model: &str,
        _req: &granary_services::ai::base::ChatRequest,
    ) -> Result<granary_services::ai::base::ChatResult, granary_services::ai::base::AiError> {
        unreachable!("embeddings only")
    }
    async fn embed(
        &self,
        _model: &str,
        texts: &[String],
    ) -> Result<Vec<Vec<f64>>, granary_services::ai::base::AiError> {
        Ok(texts.iter().map(|t| toy_vector(t)).collect())
    }
}

/// `_vector(text)`: 768 dims from the alphabet of the words.
fn toy_vector(text: &str) -> Vec<f64> {
    let mut vec = vec![0.0f64; 768];
    for word in dedupe::normalize_name(Some(text)).split_whitespace() {
        for ch in word.chars() {
            vec[(ch as usize) % 768] += 1.0;
        }
    }
    let norm = vec.iter().map(|v| v * v).sum::<f64>().sqrt();
    let norm = if norm == 0.0 { 1.0 } else { norm };
    vec.into_iter().map(|v| v / norm).collect()
}

async fn load_org(c: &mut PgConnection, id: Uuid) -> dedupe::Rec {
    let row: granary_db::rows::crm::Organization =
        sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    dedupe::Rec::Organization(row)
}

/// test_crm_bulk_ai.py `test_a_renamed_record_is_re_embedded` (the `embeddings` fixture: get_provider gives the
/// toy provider; the configured embedding route gives the model).
#[tokio::test]
async fn test_a_renamed_record_is_re_embedded() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let seam = granary_services::ai::registry::seam::Seam::install().await;
    seam.get_provider(|_, _| {
        Ok(std::sync::Arc::new(StubEmbed)
            as std::sync::Arc<
                dyn granary_services::ai::base::LLMProvider,
            >)
    });
    let id = org(&mut c, "Northwind Apex Meridian", None, None, None).await;
    let fresh = load_org(&mut c, id).await;
    assert_eq!(
        dedupe::refresh_embeddings(&mut c, "organization", std::slice::from_ref(&fresh))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        dedupe::refresh_embeddings(&mut c, "organization", &[fresh])
            .await
            .unwrap(),
        0,
        "unchanged: no re-embed"
    );
    let renamed = "Northwind Apex Pacific".to_string();
    sqlx::query("UPDATE organizations SET name = $2 WHERE id = $1")
        .bind(id)
        .bind(&renamed)
        .execute(&mut *c)
        .await
        .unwrap();
    let fresh = load_org(&mut c, id).await;
    assert_eq!(
        dedupe::refresh_embeddings(&mut c, "organization", &[fresh])
            .await
            .unwrap(),
        1
    );
    let stored: Vec<String> =
        sqlx::query_scalar("SELECT label FROM crm_embeddings WHERE record_id = $1")
            .bind(id)
            .fetch_all(&mut *c)
            .await
            .unwrap();
    assert_eq!(stored, vec![renamed]);
    drop(seam);
}
