//! Tests for crates/services/src/ai/scoring.rs (backend/app/services/ai/scoring.py; no Python test calls
//! it directly, the endpoint is a thin wrapper). The prompt is checked against the text Python builds
//! for the same lead (recorded by hand from `_gather` on the identical rows).
mod support;

use chrono::Utc;
use granary_core::auth::AuthUser;
use granary_db::rows::crm::Lead;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::ai::scoring;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

fn user() -> AuthUser {
    AuthUser {
        id: Uuid::new_v4(),
        email: "s@pytest.test".into(),
        name: "S".into(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: true,
        is_superadmin: true,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        perms: Default::default(),
    }
}

async fn lead(db: &mut PgConnection) -> Lead {
    let org = Uuid::new_v4();
    let contact = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, 'Globex Corp', false, '{}')")
        .bind(org)
        .execute(&mut *db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, email, is_deleted, custom) VALUES ($1, 'Robert', NULL, 'robert@globex.test', false, '{}')")
        .bind(contact)
        .execute(&mut *db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO leads (id, source, title, status, priority, estimated_value, currency, interested_products, \
         description, organization_id, contact_id, is_deleted, custom) VALUES ($1, 'whatsapp', 'Need 20 laptops', 'new', \
         NULL, 15000000.00, 'JPY', ARRAY['Laptops','Support plans'], 'New office in Nagoya', $2, $3, false, '{}')",
    )
    .bind(id)
    .bind(org)
    .bind(contact)
    .execute(&mut *db)
    .await
    .unwrap();
    for (when, kind, dir, subject, content, outcome, by_contact) in [
        (
            "2026-03-01T23:30:00Z",
            "whatsapp",
            Some("inbound"),
            None,
            Some("Hello, we need a quotation"),
            None,
            false,
        ),
        (
            "2026-03-02T01:00:00Z",
            "call",
            None,
            Some("Intro call"),
            None,
            Some("interested"),
            true,
        ),
    ] {
        sqlx::query(
            "INSERT INTO activities (id, entity_type, entity_id, activity_type, direction, subject, content, outcome, \
             lead_id, contact_id, created_at, is_deleted) VALUES ($1, 'lead', $2, $3, $4, $5, $6, $7, $8, $9, $10::timestamptz, false)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(kind)
        .bind(dir)
        .bind(subject)
        .bind(content)
        .bind(outcome)
        .bind(if by_contact { None } else { Some(id) })
        .bind(if by_contact { Some(contact) } else { None })
        .bind(when)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

const GATHERED: &str = "Lead inquiry: Need 20 laptops\n\
Status: new \u{b7} Priority: None \u{b7} Source: whatsapp\n\
Estimated value: JPY 15000000.00\n\
Interested in: Laptops, Support plans\n\
Notes: New office in Nagoya\n\
Organization: Globex Corp\n\
Contact: Robert  <robert@globex.test>\n\
\nConversation & activity history (most recent first):\n\
- [2026-03-02] call/None \u{2014} Intro call (outcome: interested)\n\
- [2026-03-01] whatsapp/inbound: Hello, we need a quotation";

#[tokio::test]
async fn the_lead_is_scored_saved_and_normalised() {
    let pool = support::fresh_db("ai_scoring").await;
    let mut db = pool.acquire().await.unwrap();
    let mut l = lead(&mut db).await;
    assert_eq!(scoring::_gather(&mut db, &l).await.unwrap(), GATHERED);

    let brain = Brain::new();
    brain.set(
        "scoring",
        Answer::json(
            json!({"score": "85", "temperature": "HOT", "rationale": "Budget given.",
        "signals": ["budget"]}),
        ),
    );
    let _s = brain.install().await;
    let u = user();
    let out = scoring::score_lead(&mut db, &u, &mut l).await.unwrap();
    assert_eq!(out["score"], json!(85));
    assert_eq!(out["temperature"], json!("hot"));
    assert_eq!(out["recommended_next_action"], json!(""));
    assert_eq!(out["scored_by"], json!(u.id.to_string()));
    let (_, call) = brain.calls().remove(0);
    let req = call.chat().unwrap();
    assert_eq!(req.system, scoring::_SYSTEM);
    assert_eq!(req.temperature, Some(0.2));
    let want = format!(
        "SCORING RUBRIC:\n{}\n\nLEAD DATA:\n{GATHERED}\n\nReturn the JSON score now.",
        scoring::_DEFAULT_RUBRIC
    );
    assert_eq!(req.messages, vec![json!({"role": "user", "content": want})]);
    let saved: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT ai_score FROM leads WHERE id = $1")
            .bind(l.id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(saved, Some(out));

    brain.set("scoring", Answer::text("no json here"));
    let out = scoring::score_lead(&mut db, &u, &mut l).await.unwrap();
    assert_eq!(
        out,
        json!({"error": "The model did not return a valid score. Try again."})
    );
    brain.set(
        "scoring",
        Answer::json(json!({"score": 10, "temperature": "lukewarm"})),
    );
    let out = scoring::score_lead(&mut db, &u, &mut l).await.unwrap();
    assert_eq!(out["temperature"], json!("warm"));
}
