//! Port of backend/tests/test_bug_hunt_fixes.py::test_a_sent_quotation_past_its_validity_expires_once_with_one_nudge.
//!
//! `expire_due` emits `quotation.expired` through services/nary/events.py, ported in wave 3: until then the
//! test is ignored (the call is a `todo!` stub in later/quotation_expiry.rs).

mod support;

use chrono::{NaiveDate, TimeDelta};
use granary_core::clock::today_local;
use granary_services::quotation_expiry;
use sqlx::PgConnection;
use uuid::Uuid;

async fn person(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Pytest Person', 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("pytest.{}@granary.test", Uuid::new_v4().simple()))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// `_sent_quote(valid_until, deal=True)`: a sent quotation (with a deal owned by its creator) and the open
/// "No answer on quotation" reminder the follow-up rules would have raised.
async fn sent_quote(
    db: &mut PgConnection,
    valid_until: NaiveDate,
    deal: bool,
) -> (Uuid, String, Option<Uuid>) {
    let owner = person(db).await;
    let deal_id = if deal {
        let d = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO deals (id, title, stage, value, assigned_to_id, is_deleted) VALUES ($1, $2, 'proposal', 1, $3, false)",
        )
        .bind(d)
        .bind(format!("Pytest deal {}", &Uuid::new_v4().simple().to_string()[..6]))
        .bind(owner)
        .execute(&mut *db)
        .await
        .unwrap();
        Some(d)
    } else {
        None
    };
    let q = Uuid::new_v4();
    let number = format!("PYTEST-Q-{}", &Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, created_by_id, deal_id, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', 'sent', current_date, $3, 20000000, 20000000, '{\"text\": \"NET30\"}', $4, $5, false)",
    )
    .bind(q)
    .bind(&number)
    .bind(valid_until)
    .bind(owner)
    .bind(deal_id)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO crm_reminders (id, owner_user_id, created_by_id, source, entity_type, entity_id, title, due_at, \
         original_due_at, status, snooze_count, rule_key) \
         VALUES ($1, $2, $2, 'quotation', 'quotation', $3, $4, now(), now(), 'open', 0, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(owner)
    .bind(q)
    .bind(format!("No answer on quotation {number}"))
    .bind(format!("quotation:{q}"))
    .execute(&mut *db)
    .await
    .unwrap();
    (q, number, deal_id)
}

#[tokio::test]
async fn test_a_sent_quotation_past_its_validity_expires_once_with_one_nudge() {
    let pool = support::fresh_db("s7_quotation_expiry").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let today = today_local(db).await.unwrap();
    let (q, q_no, q_deal) = sent_quote(db, today - TimeDelta::days(3), true).await;
    let (long_ago, long_no, long_deal) = sent_quote(db, today - TimeDelta::days(60), true).await;
    let (fresh, fresh_no, _) = sent_quote(db, today + TimeDelta::days(5), true).await;

    let done = quotation_expiry::expire_due(db, None).await.unwrap();
    let again = quotation_expiry::expire_due(db, None).await.unwrap();
    assert!(
        done.contains(&q_no)
            && done.contains(&long_no)
            && !done.contains(&fresh_no)
            && again.is_empty()
    );

    let status = |id: Uuid| {
        sqlx::query_scalar::<_, String>("SELECT status FROM quotations WHERE id = $1").bind(id)
    };
    assert_eq!(status(q).fetch_one(&mut *db).await.unwrap(), "expired");
    assert_eq!(
        status(long_ago).fetch_one(&mut *db).await.unwrap(),
        "expired"
    );
    assert_eq!(status(fresh).fetch_one(&mut *db).await.unwrap(), "sent");

    let old: Vec<String> =
        sqlx::query_scalar("SELECT status FROM crm_reminders WHERE entity_id = ANY($1)")
            .bind(vec![q, long_ago])
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert_eq!(old, vec!["cancelled", "cancelled"]);
    let nudge: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM crm_reminders WHERE entity_id = $1 AND status = 'open'",
    )
    .bind(q_deal)
    .fetch_all(&mut *db)
    .await
    .unwrap();
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM activities WHERE deal_id = $1 AND activity_type = 'quotation_expired'",
    )
    .bind(q_deal)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(nudge.len() == 1 && nudge[0].contains("re-quote or close") && logged == 1);
    let quiet: i64 = sqlx::query_scalar("SELECT count(*) FROM crm_reminders WHERE entity_id = $1")
        .bind(long_deal)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(quiet, 0); // ended 60 days ago: expired without a reminder
}
