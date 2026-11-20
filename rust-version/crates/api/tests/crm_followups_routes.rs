//! routers/crm_followups.py through the real app. The HTTP suite (compat-tests/http-suite/tests/
//! test_crm_followups.py) converts the API tests of backend/tests/test_crm_followups.py and pins every
//! route on both stacks; this file checks the rows the routes write.

mod support;

use chrono::{DateTime, Utc};
use serde_json::json;
use support::app;
use uuid::Uuid;

const TAG: &str = "crm_followups";

type ReminderRow = (
    Uuid,
    Option<Uuid>,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    String,
);

#[tokio::test]
async fn create_snooze_done_write_the_row() {
    let app = app(TAG);
    let me = app.user_with(&["approvals:view"]).await;
    let entity = Uuid::new_v4();
    let r = app
        .post(
            &me.token,
            "/api/v1/crm/reminders",
            json!({"entity_type": "lead", "entity_id": entity, "title": "Call",
                   "note": "n", "due_at": "2099-01-02T03:04:05+09:00"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["due_at"], json!("2099-01-02T03:04:05+09:00"));
    assert_eq!(r.json()["link"], json!(format!("/crm/leads/{entity}")));
    let id: Uuid = r.json()["id"].as_str().unwrap().parse().unwrap();
    let mut db = app.db().await;
    let row: ReminderRow = sqlx::query_as(
        "SELECT owner_user_id, created_by_id, source, due_at, original_due_at, status FROM crm_reminders WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let due: DateTime<Utc> = "2099-01-01T18:04:05Z".parse().unwrap();
    assert_eq!(
        row,
        (
            me.id,
            Some(me.id),
            "manual".into(),
            due,
            Some(due),
            "open".into()
        )
    );

    sqlx::query("UPDATE crm_reminders SET notified_at = now(), escalated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut db)
        .await
        .unwrap();
    let s = app
        .post(
            &me.token,
            &format!("/api/v1/crm/reminders/{id}/snooze"),
            json!({"days": 3}),
        )
        .await;
    assert_eq!(s.status, 200, "{}", s.text());
    assert_eq!(s.json()["due_at"], json!("2099-01-04T18:04:05+00:00"));
    let (count, notified, escalated): (i32, Option<DateTime<Utc>>, Option<DateTime<Utc>>) =
        sqlx::query_as(
            "SELECT snooze_count, notified_at, escalated_at FROM crm_reminders WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(
        (count, notified, escalated),
        (1, None, None),
        "it chimes again at the new moment"
    );

    let d = app
        .post(
            &me.token,
            &format!("/api/v1/crm/reminders/{id}/done"),
            json!(null),
        )
        .await;
    assert_eq!(d.json()["status"], json!("done"));
    let (status, by): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, completed_by_id FROM crm_reminders WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((status.as_str(), by), ("done", Some(me.id)));
}

#[tokio::test]
async fn a_team_viewer_can_close_someone_elses_reminder() {
    let app = app(TAG);
    let me = app.user_with(&["approvals:view"]).await;
    let lead = app.user_with(&["crm:view_all"]).await;
    let r = app
        .post(
            &me.token,
            "/api/v1/crm/reminders",
            json!({"entity_type": "whatsapp_chat", "chat_id": "8190@c.us", "title": "x"}),
        )
        .await;
    let id = r.json()["id"].as_str().unwrap().to_string();
    let c = app
        .post(
            &lead.token,
            &format!("/api/v1/crm/reminders/{id}/cancel"),
            json!(null),
        )
        .await;
    assert_eq!(c.status, 200);
    assert_eq!(c.json()["status"], json!("cancelled"));
}
