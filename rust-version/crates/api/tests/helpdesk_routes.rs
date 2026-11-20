//! routers/helpdesk.py through the real app. No backend test covers these routes; the full pin (both stacks)
//! is compat-tests/http-suite/tests/test_helpdesk_routes.py. Here: the SLA deadlines a public ticket gets and
//! the breach flags recorded on a late close (rows changed directly).

mod support;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "helpdesk_routes";

#[tokio::test]
async fn public_ticket_gets_the_team_sla_and_a_late_close_records_breaches() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let team = app
        .post(
            &sa.token,
            "/api/v1/helpdesk/teams",
            json!({"name": u("Team")}),
        )
        .await
        .json();
    let tid = team["id"].as_str().unwrap();
    let p = app
        .post(
            &sa.token,
            "/api/v1/helpdesk/sla-policies",
            json!({"team_id": tid, "name": "Hi", "priority": "high", "response_hours": "0.5", "resolution_hours": 8}),
        )
        .await;
    assert_eq!(p.status, 201, "{}", p.text());
    let sub = app
        .post(
            "",
            "/api/v1/helpdesk/public/tickets",
            json!({"customer_name": " C ", "customer_email": " C@X.TEST ", "subject": "S", "priority": "high",
                   "team_id": tid}),
        )
        .await;
    assert_eq!(sub.status, 201, "{}", sub.text());
    let token = sub.json()["public_token"].as_str().unwrap().to_string();
    let t = app
        .get("", &format!("/api/v1/helpdesk/public/tickets/{token}"))
        .await
        .json();
    assert_eq!(t["customer_email"], "c@x.test");
    assert_eq!(t["customer_name"], "C");
    let parse = |k: &str| t[k].as_str().unwrap().parse::<DateTime<Utc>>().unwrap();
    assert_eq!(
        parse("resolution_deadline") - parse("response_deadline"),
        Duration::minutes(450)
    );
    assert!(t.get("sla_response_breached").is_none() && t.get("assignee_id").is_none());

    let mut db = app.db().await;
    let id: Uuid = sqlx::query_scalar(
        "UPDATE helpdesk_tickets SET response_deadline = now() - interval '2 hours', \
         resolution_deadline = now() - interval '1 hour' WHERE public_token = $1::uuid RETURNING id",
    )
    .bind(&token)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let closed = app
        .patch(
            &sa.token,
            &format!("/api/v1/helpdesk/tickets/{id}"),
            json!({"status": "closed"}),
        )
        .await
        .json();
    assert_eq!(closed["sla_response_breached"], true);
    assert_eq!(closed["sla_resolution_breached"], true);
    let flags: (Option<bool>, Option<bool>) = sqlx::query_as(
        "SELECT first_response_breached, resolution_breached FROM helpdesk_tickets WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(flags, (Some(true), Some(true)));
}
