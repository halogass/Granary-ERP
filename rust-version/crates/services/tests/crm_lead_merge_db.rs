//! backend/tests/test_bug_hunt_fixes.py `test_two_open_leads_of_one_person_become_one_merge_card_and_merging_
//! moves_everything`: find_pairs, propose_all (through nary.executor.propose as the Nary actor) and merge.

mod crm_s16_common;

use chrono::{TimeDelta, Utc};
use crm_s16_common::{db, u};
use granary_services::crm::lead_merge;
use serde_json::json;
use uuid::Uuid;

const TAG: &str = "s16_lead_merge";

#[tokio::test]
async fn test_two_open_leads_of_one_person_become_one_merge_card_and_merging_moves_everything() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let contact = Uuid::new_v4();
    sqlx::query("INSERT INTO contacts (id, first_name, email, is_deleted, custom) VALUES ($1, $2, $3, false, '{}')")
        .bind(contact)
        .bind(u("Ress"))
        .bind(format!("{}@ress.test", u("r")))
        .execute(&mut *c)
        .await
        .unwrap();
    let (old, dup) = (Uuid::new_v4(), Uuid::new_v4());
    for (id, title, source, status, created) in [
        (
            old,
            "Northwind partnership",
            "whatsapp",
            "qualifying",
            Utc::now() - TimeDelta::days(2),
        ),
        (dup, "ress ai: Laptop", "website_form", "new", Utc::now()),
    ] {
        sqlx::query(
            "INSERT INTO leads (id, title, source, status, contact_id, created_at, updated_at, raw_message, is_deleted, custom) \
             VALUES ($1, $2, $3, $4, $5, $6, $6, $7, false, '{}')",
        )
        .bind(id)
        .bind(title)
        .bind(source)
        .bind(status)
        .bind(contact)
        .bind(created)
        .bind(if id == dup { Some("form text") } else { None })
        .execute(&mut *c)
        .await
        .unwrap();
    }
    let cid = format!("62{}@c.us", &Uuid::new_v4().as_u128().to_string()[..10]);
    sqlx::query("INSERT INTO wa_chats (id, chat_id, status, contact_id, lead_id, is_my_contact) VALUES ($1, $2, 'active', $3, $4, false)")
        .bind(Uuid::new_v4())
        .bind(&cid)
        .bind(contact)
        .bind(dup)
        .execute(&mut *c)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, type, body, wa_ts, lead_id, origin, has_media) \
         VALUES ($1, $2, $3, 'inbound', false, 'chat', 'Hi', now(), $4, 'live', false)",
    )
    .bind(Uuid::new_v4())
    .bind(&Uuid::new_v4().simple().to_string()[..20])
    .bind(&cid)
    .bind(dup)
    .execute(&mut *c)
    .await
    .unwrap();
    sqlx::query("INSERT INTO activities (id, entity_type, entity_id, lead_id, activity_type, subject, direction) VALUES ($1, 'lead', $2, $2, 'note', 'form', 'outbound')")
        .bind(Uuid::new_v4())
        .bind(dup)
        .execute(&mut *c)
        .await
        .unwrap();

    let pairs = lead_merge::find_pairs(&mut c).await.unwrap();
    let mine: Vec<_> = pairs.iter().filter(|(_, drop, _)| drop.id == dup).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].0.id, old);
    assert_eq!(mine[0].2, "same contact");

    let actor = granary_services::nary::roles::nary_actor(&mut c)
        .await
        .unwrap()
        .expect("the Nary actor");
    let made = lead_merge::propose_all(&mut c, &actor).await.unwrap();
    let cards: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM agent_pending_actions WHERE action = 'lead.merge' AND payload ->> 'drop_id' = $1",
    )
    .bind(dup.to_string())
    .fetch_all(&mut *c)
    .await
    .unwrap();
    assert!(made >= 1);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["keep_id"], old.to_string());
    assert!(lead_merge::find_pairs(&mut c)
        .await
        .unwrap()
        .iter()
        .all(|(_, drop, _)| drop.id != dup)); // never raised twice

    let (mut keep, mut drop) = (mine[0].0.clone(), mine[0].1.clone());
    let moved = lead_merge::merge(&mut c, &mut keep, &mut drop)
        .await
        .unwrap();
    assert_eq!(moved["chats"], json!([cid]));
    assert_eq!(moved["lead_id"], dup.to_string());
    assert_eq!(moved["reminders_closed"], 0);

    let chat_lead: Option<Uuid> =
        sqlx::query_scalar("SELECT lead_id FROM wa_chats WHERE chat_id = $1")
            .bind(&cid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    let msg_lead: Option<Uuid> =
        sqlx::query_scalar("SELECT lead_id FROM wa_messages WHERE chat_id = $1")
            .bind(&cid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    let acts: i64 = sqlx::query_scalar("SELECT count(*) FROM activities WHERE subject = 'form' AND lead_id = $1 AND entity_id = $1")
        .bind(old)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!((chat_lead, msg_lead, acts), (Some(old), Some(old), 1));
    let (status, reason): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT status, disqualification_reason FROM leads WHERE id = $1")
            .bind(dup)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(
        (status.as_deref(), reason.as_deref()),
        (Some("disqualified"), Some("duplicate"))
    );
    let (raw_message, data): (Option<String>, serde_json::Value) =
        sqlx::query_as("SELECT raw_message, raw_channel_data FROM leads WHERE id = $1")
            .bind(old)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(raw_message.as_deref(), Some("form text"));
    assert_eq!(data["merged"][0]["lead_id"], dup.to_string());
}
