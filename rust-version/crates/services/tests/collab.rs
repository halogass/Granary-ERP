//! services/collab.rs: record_activity rows (stringified, truncated, None/"" as NULL) and ACTIVITY_FIELDS.

mod support;

use granary_compat::pyjson::PyValue;
use granary_services::collab::{activity_field, activity_value, record_activity, ACTIVITY_FIELDS};

#[test]
fn values_are_python_str_truncated() {
    assert_eq!(activity_value(None), None);
    assert_eq!(activity_value(Some(&PyValue::None)), None);
    assert_eq!(activity_value(Some(&PyValue::Str(String::new()))), None);
    assert_eq!(activity_value(Some(&PyValue::Int(0))).as_deref(), Some("0"));
    assert_eq!(
        activity_value(Some(&PyValue::Bool(false))).as_deref(),
        Some("False")
    );
    assert_eq!(
        activity_value(Some(&PyValue::Str(" ".into()))).as_deref(),
        Some(" ")
    );
    let long: String = "é".repeat(600);
    assert_eq!(
        activity_value(Some(&PyValue::Str(long)))
            .unwrap()
            .chars()
            .count(),
        500
    );
    let u = uuid::Uuid::new_v4();
    assert_eq!(activity_value(Some(&PyValue::Uuid(u))), Some(u.to_string()));
    assert_eq!(ACTIVITY_FIELDS.len(), 4);
    assert_eq!(activity_field("assigned_to_id"), Some("assignee"));
    assert_eq!(activity_field("assignee_id"), Some("assignee"));
    assert_eq!(activity_field("status"), Some("status"));
    assert_eq!(activity_field("priority"), Some("priority"));
    assert_eq!(activity_field("title"), None);
}

#[tokio::test]
async fn record_activity_writes_one_row() {
    let pool = support::fresh_db("collab").await;
    let mut tx = pool.begin().await.unwrap();
    let entity = uuid::Uuid::new_v4().to_string();
    record_activity(
        &mut tx,
        "project_task",
        &entity,
        None,
        "status",
        Some("status"),
        Some(&PyValue::Str("todo".into())),
        Some(&PyValue::Str(String::new())),
    )
    .await
    .unwrap();
    record_activity(
        &mut tx,
        "project_task",
        &entity,
        None,
        "created",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let rows: Vec<(String, String, Option<uuid::Uuid>, String, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT entity_type, entity_id, actor_id, verb, field, old_value, new_value FROM issue_activity \
                        WHERE entity_id = $1 ORDER BY verb DESC")
            .bind(&entity)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        (
            "project_task".into(),
            entity.clone(),
            None,
            "status".into(),
            Some("status".into()),
            Some("todo".into()),
            None
        )
    );
    assert_eq!(
        rows[1],
        (
            "project_task".into(),
            entity.clone(),
            None,
            "created".into(),
            None,
            None,
            None
        )
    );
    tx.rollback().await.unwrap();
}
