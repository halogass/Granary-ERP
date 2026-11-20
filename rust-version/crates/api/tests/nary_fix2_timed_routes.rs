//! Nary fix batch 2 (agent T3, A5): POST /nary/send with send_at keeps the message as a timed card (sent by
//! the playbooks tick at its minute); a time in the past is refused with the reason.

mod support;

use serde_json::json;
use support::app;

const TAG: &str = "nary_fix2_timed_routes";

#[tokio::test]
async fn test_send_with_send_at_schedules_instead_of_sending() {
    let app = app(TAG);
    let _guard = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let tz = granary_core::clock::local_tz(&mut db).await.unwrap();
    let at = |minutes: i64| {
        (chrono::Utc::now() + chrono::Duration::minutes(minutes))
            .with_timezone(&tz)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    };
    let body = |send_at: String| {
        json!({"channel": "email", "to": "bob@customer.test", "subject": "Meeting reminder",
               "text": "Good morning Mr Bob,\n\nA reminder about our meeting.", "send_at": send_at})
    };
    let r = app.post(&sa.token, "/api/v1/nary/send", body(at(-5))).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("not in the future"), "{}", r.text());

    let r = app.post(&sa.token, "/api/v1/nary/send", body(at(45))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let card = r.json();
    assert_eq!(card["status"], json!("scheduled"), "{card}");
    assert!(
        card["label"]
            .as_str()
            .unwrap()
            .starts_with("Scheduled for "),
        "{card}"
    );
    let id: uuid::Uuid = card["id"].as_str().unwrap().parse().unwrap();
    let (status, result, by_owner): (String, serde_json::Value, Option<String>) = sqlx::query_as(
        "SELECT status, result, payload ->> 'by_owner' FROM agent_pending_actions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(status, "scheduled");
    assert_eq!(result["send_at"], json!(true));
    assert!(result["when"].as_str().unwrap().contains(&at(45)[11..16]));
    assert_eq!(by_owner.as_deref(), Some("true"));
}
