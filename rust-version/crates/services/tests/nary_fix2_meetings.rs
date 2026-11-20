//! Nary fix batch 2 (agent T3): meeting results reach the DEAL (the Owner's 14-19 Oct review, A6).
//!
//! - A lead promoted to a deal: its open meetings and follow-ups move to the deal; a reminder made later for
//!   the old lead lands on the deal; a debrief of a meeting still filed on the lead puts the results, the
//!   commitments' follow-ups and a note on the deal's timeline, links the meeting to the deal, and proposes
//!   the stage and value the call settled as a crm.record_change card.
//! - get_activity_history: the deal's history with its source lead (meetings, results, notes, follow-ups,
//!   stage changes, WhatsApp), paged, with the record's visibility.

mod nary_n10_common;
mod support;

use granary_db::rows::nary::NaryMeeting;
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use granary_services::ai::tools::catalog;
use granary_services::crm::{followups, promote};
use granary_services::nary::{meetings, rhythm};
use nary_n10_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

async fn setup(tag: &str, brain: &Brain) -> (sqlx::pool::PoolConnection<sqlx::Postgres>, Seam) {
    let seam = brain.install().await;
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    rhythm::set_working_time_override(Some(true));
    (pool.acquire().await.unwrap(), seam)
}

async fn booked_meeting(db: &mut PgConnection, cust: &Cust, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_meetings (id, lead_id, contact_id, chat_id, channel, status, slots, origin, attendees, \
         title, starts_at, meet_link) VALUES ($1, $2, $3, $4, 'whatsapp', 'booked', '[]', 'chat', '[]', $5, \
         now() - interval '2 hours', 'https://meet.google.com/abc-defg-hij')",
    )
    .bind(id)
    .bind(cust.lead_id)
    .bind(cust.contact_id)
    .bind(&cust.chat_id)
    .bind(title)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn reminders_on(db: &mut PgConnection, entity_id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT entity_type, title FROM crm_reminders WHERE entity_id = $1 ORDER BY title",
    )
    .bind(entity_id)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

fn extract(deal_update: Value) -> Value {
    json!({
        "summary": "Globex wants 12 laptops for two offices; budget approved by the office manager.",
        "bant": {"budget": 3, "authority": 2, "need": 3, "timeline": 2},
        "requirements": ["disk encryption", "two offices"],
        "commitments": [
            {"who": "us", "what": "Send the solution proposal", "due": "2026-10-23"},
            {"who": "customer", "what": "Send the site photos", "due": null}],
        "objections": [], "next_steps": ["proposal review on Friday"], "language": "en",
        "deal_update": deal_update})
}

async fn promoted(db: &mut PgConnection, title: &str) -> (Cust, Uuid, Uuid, Uuid) {
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let cust = customer(db, Some("robert.globex@customer.test"), true, "en").await;
    let lead_id = cust.lead_id.unwrap();
    let meeting_id = booked_meeting(db, &cust, title).await;
    let mut r = followups::NewReminder::new(me, "lead", "Call back about the demo");
    r.entity_id = Some(lead_id);
    followups::create(db, r).await.unwrap();
    let out = promote::promote_lead(db, lead_id, &user, "promote")
        .await
        .unwrap();
    let deal_id: Uuid = out["deal_id"].as_str().unwrap().parse().unwrap();
    (cust, me, meeting_id, deal_id)
}

#[tokio::test]
async fn test_a_promoted_leads_meeting_results_reach_the_deal() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("nary_fix2_meet_deal", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let (cust, me, meeting_id, deal_id) = promoted(db, "Demo Globex").await;
    let lead_id = cust.lead_id.unwrap();
    // promotion moved what was open on the lead
    let m: NaryMeeting = sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
        .bind(meeting_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(m.deal_id, Some(deal_id));
    assert_eq!(
        reminders_on(db, deal_id).await,
        vec![("deal".to_string(), "Call back about the demo".to_string())]
    );
    // a reminder made later for the old lead lands on the deal
    let mut r = followups::NewReminder::new(me, "lead", "Ask about the budget");
    r.entity_id = Some(lead_id);
    let made = followups::create(db, r).await.unwrap().unwrap();
    assert_eq!(
        (made.entity_type.as_str(), made.entity_id),
        ("deal", Some(deal_id))
    );
    assert!(reminders_on(db, lead_id).await.is_empty());

    // a meeting still filed on the lead only (booked before the fix): the debrief finds the deal
    sqlx::query("UPDATE nary_meetings SET deal_id = NULL WHERE id = $1")
        .bind(meeting_id)
        .execute(&mut *db)
        .await
        .unwrap();
    let meeting: NaryMeeting = sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
        .bind(meeting_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    brain.set(
        "nary.transcript_extract",
        Answer::json(extract(json!({"stage": "proposal", "value": 250000000, "currency": "JPY",
                                    "reason": "They asked for the proposal and confirmed the budget."}))),
    );
    brain.set(
        "nary.summary_email",
        Answer::json(json!({"subject": "Meeting summary", "body": "Thank you for your time..."})),
    );
    let user = auth_user(db, me).await;
    let out = meetings::debrief(
        db,
        Some(&meeting),
        None,
        "transcript...",
        "meeting transcript",
        &user,
    )
    .await
    .unwrap();
    assert_eq!(out["commitments"], json!(1));
    let deal_reminders = reminders_on(db, deal_id).await;
    assert!(
        deal_reminders.contains(&("deal".to_string(), "Send the solution proposal".to_string())),
        "{deal_reminders:?}"
    );
    assert!(
        reminders_on(db, lead_id).await.is_empty(),
        "nothing on the old lead"
    );
    let (deal_of_meeting, status): (Option<Uuid>, String) =
        sqlx::query_as("SELECT deal_id, status FROM nary_meetings WHERE id = $1")
            .bind(meeting_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!((deal_of_meeting, status.as_str()), (Some(deal_id), "done"));
    let insight_deal: Option<Uuid> =
        sqlx::query_scalar("SELECT deal_id FROM nary_insights WHERE meeting_id = $1")
            .bind(meeting_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(insight_deal, Some(deal_id));
    let (etype, subject, content): (String, String, String) = sqlx::query_as(
        "SELECT entity_type, subject, content FROM activities WHERE deal_id = $1 AND activity_type = 'note'",
    )
    .bind(deal_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(etype, "deal");
    assert_eq!(subject, "Meeting results: Demo Globex");
    assert!(
        content.contains("12 laptops") && content.contains("Next steps: proposal review on Friday")
    );
    assert!(
        content.contains("us: Send the solution proposal (by 2026-10-23)"),
        "{content}"
    );
    let bant: Value =
        sqlx::query_scalar("SELECT custom -> 'nary_call' -> 'bant' FROM deals WHERE id = $1")
            .bind(deal_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(bant["budget"], json!(3));

    // the stage and value the call settled wait for the Owner as a card
    let card_id: Uuid = out["deal_update_card"].as_str().unwrap().parse().unwrap();
    let (action, payload, rationale, status): (String, Value, Option<String>, String) =
        sqlx::query_as(
            "SELECT action, payload, rationale, status FROM agent_pending_actions WHERE id = $1",
        )
        .bind(card_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(action, "crm.record_change");
    assert_eq!(payload["object_type"], json!("deal"));
    assert_eq!(payload["record_id"], json!(deal_id.to_string()));
    assert_eq!(payload["fields"]["stage"], json!("proposal"));
    assert_eq!(payload["fields"]["value"].as_f64(), Some(250000000.0));
    // the lead had no currency, so the deal took the default; the agreed price was in yen
    let currency: Option<String> = sqlx::query_scalar("SELECT currency FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_ne!(currency.as_deref(), Some("JPY"));
    assert_eq!(payload["fields"]["currency"], json!("JPY"));
    assert!(rationale.unwrap().contains("confirmed the budget"));
    assert_ne!(status, "applied", "a person decides");
    let stage: String = sqlx::query_scalar("SELECT stage FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(
        stage, "qualifying",
        "nothing changed before the card is applied"
    );

    // the deal's history shows it all, its source lead included
    let h = catalog::run_tool(
        db,
        &user,
        "get_activity_history",
        json!({"object_type": "deal", "record_id": deal_id.to_string()}),
    )
    .await;
    assert_eq!(h["from_lead_id"], json!(lead_id.to_string()), "{h}");
    let kinds: Vec<String> = h["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap().to_string())
        .collect();
    for k in [
        "meeting_result",
        "activity",
        "follow_up",
        "meeting",
        "stage_change",
        "whatsapp",
    ] {
        assert!(kinds.contains(&k.to_string()), "{k} missing in {kinds:?}");
    }
    let titles: Vec<String> = h["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        titles.iter().any(|t| t == "Meeting results: Demo Globex"),
        "{titles:?}"
    );
    assert!(
        titles
            .iter()
            .any(|t| t.starts_with("Stage") && t.ends_with("qualifying")),
        "{titles:?}"
    );
    // paged
    let p1 = catalog::run_tool(
        db,
        &user,
        "get_activity_history",
        json!({"object_type": "deal", "record_id": deal_id.to_string(), "limit": 2}),
    )
    .await;
    assert_eq!(p1["count"], json!(2));
    assert_eq!(p1["has_more"], json!(true));
    let p2 = catalog::run_tool(
        db,
        &user,
        "get_activity_history",
        json!({"object_type": "deal", "record_id": deal_id.to_string(), "limit": 2, "offset": 2}),
    )
    .await;
    assert_ne!(p1["items"][0]["id"], p2["items"][0]["id"]);
    // the old lead points at its deal
    let lh = catalog::run_tool(
        db,
        &user,
        "get_activity_history",
        json!({"object_type": "lead", "record_id": lead_id.to_string()}),
    )
    .await;
    assert_eq!(
        lh["promoted_to_deal_id"],
        json!(deal_id.to_string()),
        "{lh}"
    );
    // the record's visibility: someone who sees only their own records can't read it
    let mut other = user.clone();
    other.id = Uuid::new_v4();
    other.is_superadmin = false;
    other.perms = ["crm:view_own".to_string()].into_iter().collect();
    let denied = catalog::run_tool(
        db,
        &other,
        "get_activity_history",
        json!({"object_type": "deal", "record_id": deal_id.to_string()}),
    )
    .await;
    assert!(
        denied["error"]
            .as_str()
            .unwrap()
            .contains("assigned to someone else"),
        "{denied}"
    );
    rhythm::set_working_time_override(None);
}

#[tokio::test]
async fn test_no_deal_card_when_the_call_settled_nothing_new() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("nary_fix2_meet_nochange", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let (_cust, me, meeting_id, deal_id) = promoted(db, "Follow-up call").await;
    let meeting: NaryMeeting = sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
        .bind(meeting_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    // the stage it already has, no value
    brain.set(
        "nary.transcript_extract",
        Answer::json(extract(
            json!({"stage": "qualifying", "value": null, "currency": null, "reason": "Still qualifying."}),
        )),
    );
    brain.set(
        "nary.summary_email",
        Answer::json(json!({"subject": "Summary", "body": "Thank you..."})),
    );
    let user = auth_user(db, me).await;
    let out = meetings::debrief(
        db,
        Some(&meeting),
        None,
        "notes",
        "Owner voice debrief",
        &user,
    )
    .await
    .unwrap();
    assert!(out["deal_update_card"].is_null(), "{out}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_pending_actions WHERE action = 'crm.record_change' \
         AND payload ->> 'record_id' = $1",
    )
    .bind(deal_id.to_string())
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(n, 0);
    rhythm::set_working_time_override(None);
}
