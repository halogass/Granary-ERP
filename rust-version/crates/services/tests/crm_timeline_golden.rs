//! services/crm/timeline.py: build_timeline / related_counts replayed on the scenario database (merge order,
//! number and date formatting), plus test_crm_ai_api.py test_timeline_rolls_up_related_records at
//! service level.
mod crm_s15_common;
mod support;

use granary_services::crm::timeline::{build_timeline, related_counts};
use serde_json::{json, Value};
use uuid::Uuid;

#[tokio::test]
async fn timeline_golden() {
    let pool = support::fresh_db("s15_timeline").await;
    crm_s15_common::scenario_db(&pool).await;
    let user = crm_s15_common::named_user("super");
    let mut bad = Vec::new();
    for c in crm_s15_common::load("crm_timeline/timeline.json")
        .as_array()
        .unwrap()
    {
        let ot = c["object_type"].as_str().unwrap();
        let rid = Uuid::parse_str(c["record_id"].as_str().unwrap()).unwrap();
        let limit = c["limit"].as_i64().unwrap();
        let mut tx = pool.begin().await.unwrap();
        let counts = related_counts(&mut tx, ot, rid).await.unwrap();
        if counts != c["related"] {
            bad.push(format!("related {c}\n got {counts}"));
        }
        match build_timeline(&mut tx, &user, ot, rid, limit).await {
            Ok(items) => {
                if c.get("items") != Some(&Value::Array(items.clone())) {
                    bad.push(format!(
                        "items {ot} {rid} {limit}\n exp {}\n got {}",
                        c["items"],
                        json!(items)
                    ));
                }
            }
            Err(e) => {
                if c.get("exc").is_none() {
                    bad.push(format!("items {ot} {rid} {limit}: unexpected error {e}"));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );

    // test_timeline_rolls_up_related_records
    let mut conn = pool.acquire().await.unwrap();
    let org = Uuid::parse_str("00000000-0000-0000-0000-000000000101").unwrap();
    let related = related_counts(&mut conn, "organization", org)
        .await
        .unwrap();
    assert!(related["contacts"].as_i64().unwrap() >= 1 && related["deals"].as_i64().unwrap() >= 1);
    let items = build_timeline(&mut conn, &user, "organization", org, 60)
        .await
        .unwrap();
    assert!(items
        .iter()
        .any(|i| i["kind"] == "activity" && i["title"] == "Discovery"));
    // an unsupported object_type is refused (the router turns it into a 400 before calling)
    assert!(build_timeline(&mut conn, &user, "invoice", org, 60)
        .await
        .is_err());
}
