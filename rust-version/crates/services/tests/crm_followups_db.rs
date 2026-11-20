//! backend/tests/test_crm_followups.py (rules, firing, weekly digest), plus the follow-up tests of
//! test_audit_gaps_b.py, test_nary_p0_promote.py, test_nary_p0_stages.py and test_nary_chat_fixes.py.
//! Python's `sent` fixture (fake notify/push) is replaced by the notifications rows and the notify outbox.

mod crm_s16_common;

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, TimeZone, Timelike, Utc};
use chrono_tz::Asia::Tokyo;
use chrono_tz::Europe::London;
use crm_s16_common::{db, make_person, set_setting, u};
use granary_compat::text::utils::add_business_days;
use granary_services::approvals::notify;
use granary_services::crm::followups::{self, FollowupSettings, NewReminder};
use sqlx::PgConnection;
use uuid::Uuid;

const TAG: &str = "s16_followups";

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// The autouse fixture: lift the per-rule cap, keep delivery independent of the wall clock.
async fn base_settings(c: &mut PgConnection) {
    set_setting(c, "crm.followup.max_per_rule_pass", "500").await;
    set_setting(c, "crm.followup.quiet_from", "00:00").await;
    set_setting(c, "crm.followup.quiet_to", "23:59").await;
}

async fn contact(c: &mut PgConnection, owner: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, email, is_deleted, custom) VALUES ($1, $2, $3, $4, false, '{}')")
        .bind(id)
        .bind(u("Bob"))
        .bind(u("Sanders"))
        .bind(format!("{}@customer.test", u("b")))
        .execute(&mut *c)
        .await
        .unwrap();
    if let Some(owner) = owner {
        sqlx::query(
            "INSERT INTO leads (id, title, source, contact_id, assigned_to_id, status, is_deleted, custom) \
             VALUES ($1, $2, 'whatsapp', $3, $4, 'new', false, '{}')",
        )
        .bind(Uuid::new_v4())
        .bind(u("Lead"))
        .bind(id)
        .bind(owner)
        .execute(&mut *c)
        .await
        .unwrap();
    }
    id
}

async fn activity(
    c: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
    direction: &str,
    when: DateTime<Utc>,
    by: Option<Uuid>,
) {
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, activity_type, direction, subject, completed_at, is_completed, created_by_id) \
         VALUES ($1, $2, $3, 'whatsapp', $4, 'msg', $5, true, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(entity_type)
    .bind(entity_id)
    .bind(direction)
    .bind(when)
    .bind(by)
    .execute(&mut *c)
    .await
    .unwrap();
}

async fn lead(
    c: &mut PgConnection,
    title: &str,
    assigned: Option<Uuid>,
    created_by: Option<Uuid>,
    age_days: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    let at = now() - TimeDelta::days(age_days);
    sqlx::query(
        "INSERT INTO leads (id, title, source, status, assigned_to_id, created_by_id, created_at, updated_at, is_deleted, custom) \
         VALUES ($1, $2, 'referral', 'new', $3, $4, $5, $5, false, '{}')",
    )
    .bind(id)
    .bind(title)
    .bind(assigned)
    .bind(created_by)
    .bind(at)
    .execute(&mut *c)
    .await
    .unwrap();
    id
}

async fn deal(
    c: &mut PgConnection,
    stage: &str,
    assigned: Option<Uuid>,
    created_at: DateTime<Utc>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, assigned_to_id, created_at, updated_at, is_deleted, custom) \
         VALUES ($1, $2, $3, $4, $5, $5, false, '{}')",
    )
    .bind(id)
    .bind(u("Stuck deal"))
    .bind(stage)
    .bind(assigned)
    .bind(created_at)
    .execute(&mut *c)
    .await
    .unwrap();
    id
}

/// (entity_id, source, status, title, rule_key) of the reminders of an owner.
async fn reminders(
    c: &mut PgConnection,
    owner: Uuid,
    source: Option<&str>,
) -> Vec<(Option<Uuid>, String, String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT entity_id, source, status, title, rule_key FROM crm_reminders WHERE owner_user_id = $1 \
         AND ($2::text IS NULL OR source = $2) ORDER BY created_at",
    )
    .bind(owner)
    .bind(source)
    .fetch_all(&mut *c)
    .await
    .unwrap()
}

async fn generate(c: &mut PgConnection) -> Vec<(String, i64)> {
    followups::generate(c).await.unwrap()
}

fn got(made: &[(String, i64)], key: &str) -> Option<i64> {
    made.iter().find(|(k, _)| k == key).map(|(_, n)| *n)
}

// ─── Dates ───────────────────────────────────────────────────────────────────

#[test]
fn test_business_days_skip_the_weekend() {
    let friday = Utc.with_ymd_and_hms(2026, 9, 18, 10, 0, 0).unwrap();
    assert_eq!(
        add_business_days(friday, 1).date_naive(),
        NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
    );
    assert_eq!(
        add_business_days(friday, 3).date_naive(),
        NaiveDate::from_ymd_opt(2026, 9, 23).unwrap()
    );
}

#[test]
fn test_default_due_is_three_business_days_at_nine_local() {
    let cfg = FollowupSettings::default();
    let due = followups::default_due(
        &cfg,
        Tokyo,
        Some(Utc.with_ymd_and_hms(2026, 9, 18, 3, 0, 0).unwrap()),
    )
    .unwrap();
    let local = due.with_timezone(&Tokyo);
    assert_eq!(
        local.date_naive(),
        NaiveDate::from_ymd_opt(2026, 9, 23).unwrap()
    );
    assert_eq!(local.hour(), 9);
}

#[test]
fn test_quiet_hours() {
    let cfg = FollowupSettings {
        quiet_from: "08:00".into(),
        quiet_to: "20:00".into(),
        ..Default::default()
    };
    assert!(followups::in_quiet_hours(
        &cfg,
        Tokyo,
        Some(Utc.with_ymd_and_hms(2026, 9, 21, 5, 0, 0).unwrap())
    ));
    assert!(!followups::in_quiet_hours(
        &cfg,
        Tokyo,
        Some(Utc.with_ymd_and_hms(2026, 9, 21, 22, 0, 0).unwrap())
    ));
}

/// test_audit_gaps_b.py
#[tokio::test]
async fn test_followup_default_due_uses_the_timezone_setting() {
    let d = db(TAG).await;
    let mut tx = d.pool.begin().await.unwrap();
    set_setting(&mut tx, "timezone", "Asia/Tokyo").await;
    let tz = granary_core::clock::local_tz(&mut tx).await.unwrap();
    assert_eq!(tz, Tokyo);
    let cfg = FollowupSettings {
        default_delay_days: 3,
        ..Default::default()
    };
    let due = followups::default_due(
        &cfg,
        tz,
        Some(Utc.with_ymd_and_hms(2026, 9, 17, 23, 0, 0).unwrap()),
    )
    .unwrap();
    let local = due.with_timezone(&Tokyo);
    assert_eq!(
        local.date_naive(),
        NaiveDate::from_ymd_opt(2026, 9, 23).unwrap()
    );
    assert_eq!(local.hour() as i64, FollowupSettings::default().due_hour);
    let cfg = FollowupSettings {
        quiet_from: "08:00".into(),
        quiet_to: "20:00".into(),
        ..Default::default()
    };
    let at = Utc.with_ymd_and_hms(2026, 9, 21, 11, 30, 0).unwrap();
    assert!(!followups::in_quiet_hours(&cfg, tz, Some(at)));
    assert!(followups::in_quiet_hours(&cfg, London, Some(at)));
    tx.rollback().await.unwrap();
}

// ─── Rules ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_no_reply_rule_raises_one_reminder_until_it_is_answered() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let owner = make_person(&mut c, false, &[]).await;
    let ct = contact(&mut c, Some(owner.id)).await;
    activity(
        &mut c,
        "contact",
        ct,
        "outbound",
        now() - TimeDelta::days(5),
        Some(owner.id),
    )
    .await;

    assert!(got(&generate(&mut c).await, "no_reply").unwrap() >= 1);
    let mine: Vec<_> = reminders(&mut c, owner.id, Some("no_reply"))
        .await
        .into_iter()
        .filter(|r| r.0 == Some(ct))
        .collect();
    assert_eq!(mine.len(), 1);
    assert!(mine[0].3.contains("No reply"));

    generate(&mut c).await; // a second pass must not duplicate it
    assert_eq!(
        reminders(&mut c, owner.id, Some("no_reply"))
            .await
            .into_iter()
            .filter(|r| r.0 == Some(ct))
            .count(),
        1
    );

    // The customer answers: the conversation is no longer silent
    activity(
        &mut c,
        "contact",
        ct,
        "inbound",
        now() - TimeDelta::hours(1),
        None,
    )
    .await;
    sqlx::query("UPDATE crm_reminders SET status = 'done' WHERE entity_id = $1")
        .bind(ct)
        .execute(&mut *c)
        .await
        .unwrap();
    generate(&mut c).await;
    assert!(reminders(&mut c, owner.id, Some("no_reply"))
        .await
        .into_iter()
        .all(|r| !(r.0 == Some(ct) && r.2 == "open")));
}

#[tokio::test]
async fn test_a_recent_message_raises_nothing() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let owner = make_person(&mut c, false, &[]).await;
    let ct = contact(&mut c, Some(owner.id)).await;
    activity(
        &mut c,
        "contact",
        ct,
        "outbound",
        now() - TimeDelta::hours(6),
        Some(owner.id),
    )
    .await;
    generate(&mut c).await;
    assert!(reminders(&mut c, owner.id, None)
        .await
        .into_iter()
        .all(|r| r.0 != Some(ct)));
}

#[tokio::test]
async fn test_idle_lead_and_stuck_deal_rules() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let owner = make_person(&mut c, false, &[]).await;
    let lead_id = lead(&mut c, &u("Quiet lead"), Some(owner.id), None, 30).await;
    let deal_id = deal(
        &mut c,
        "proposal",
        Some(owner.id),
        now() - TimeDelta::days(30),
    )
    .await;
    generate(&mut c).await;
    let rows = reminders(&mut c, owner.id, None).await;
    let stale_lead = rows.iter().rev().find(|r| r.1 == "stale_lead").unwrap();
    let stale_deal = rows.iter().rev().find(|r| r.1 == "stale_deal").unwrap();
    assert_eq!(stale_lead.0, Some(lead_id));
    assert_eq!(stale_deal.0, Some(deal_id));
    assert!(stale_deal.3.contains("proposal"));
}

#[tokio::test]
async fn test_a_rule_that_is_switched_off_raises_nothing() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let owner = make_person(&mut c, false, &[]).await;
    let ct = contact(&mut c, Some(owner.id)).await;
    activity(
        &mut c,
        "contact",
        ct,
        "outbound",
        now() - TimeDelta::days(9),
        Some(owner.id),
    )
    .await;
    set_setting(&mut c, "crm.followup.rule_no_reply", "false").await;
    let made = generate(&mut c).await;
    set_setting(&mut c, "crm.followup.rule_no_reply", "true").await;
    assert!(got(&made, "no_reply").is_none());
    assert!(reminders(&mut c, owner.id, None)
        .await
        .into_iter()
        .all(|r| r.0 != Some(ct)));
}

#[tokio::test]
async fn test_owner_falls_back_to_the_creator_and_skips_ownerless_records() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let creator = make_person(&mut c, false, &[]).await;
    let lead_id = lead(&mut c, &u("Creator lead"), None, Some(creator.id), 30).await;
    let orphan = lead(&mut c, &u("Orphan lead"), None, None, 30).await;
    generate(&mut c).await;
    let ids: Vec<Option<Uuid>> = reminders(&mut c, creator.id, Some("stale_lead"))
        .await
        .into_iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(ids, vec![Some(lead_id)]);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM crm_reminders WHERE entity_id = $1")
        .bind(orphan)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn test_a_rule_raises_at_most_its_daily_cap() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    // other tests' quiet leads would also count: close them first so this test sees only its own
    sqlx::query("UPDATE leads SET status = 'disqualified'")
        .execute(&mut *c)
        .await
        .unwrap();
    let owner = make_person(&mut c, false, &[]).await;
    for _ in 0..4 {
        lead(&mut c, &u("Old lead"), Some(owner.id), None, 40).await;
    }
    set_setting(&mut c, "crm.followup.max_per_rule_pass", "2").await;
    let first = generate(&mut c).await;
    let second = generate(&mut c).await;
    set_setting(&mut c, "crm.followup.max_per_rule_pass", "25").await;
    assert_eq!(got(&first, "stale_lead").unwrap_or(0), 2);
    assert_eq!(got(&second, "stale_lead").unwrap_or(0), 2); // the next pass picks up where it left off
}

// ─── Firing ──────────────────────────────────────────────────────────────────

async fn make(c: &mut PgConnection, owner: Uuid, due_at: DateTime<Utc>) -> Uuid {
    let mut new = NewReminder::new(owner, "contact", "Call them back");
    new.entity_id = Some(Uuid::new_v4());
    new.due_at = Some(due_at);
    followups::create(c, new).await.unwrap().unwrap().id
}

async fn bells(c: &mut PgConnection, entity_id: &str) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT user_id, link FROM notifications WHERE entity_id = $1")
        .bind(entity_id)
        .fetch_all(&mut *c)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_due_reminder_goes_to_bell_and_whatsapp_then_email_the_next_day() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let owner = make_person(&mut c, false, &[]).await;
    let rid = make(&mut c, owner.id, now() - TimeDelta::minutes(5)).await;
    let rid_s = rid.hyphenated().to_string();

    notify::scope(async {
        let result = followups::fire_due(&mut c, None).await.unwrap();
        assert!(result.due >= 1);
        // the bell entry (email False, whatsapp False: nothing queued for it)
        let mine = bells(&mut c, &rid_s).await;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].0, owner.id);
        assert!(mine[0].1.starts_with("/crm/contacts"));
        let out = notify::pending_outbox();
        let digest: Vec<_> = out
            .iter()
            .filter(|i| i.user_ids == vec![owner.id.to_string()])
            .collect();
        assert_eq!(digest.len(), 1); // WhatsApp: one digest
        assert!(digest[0].whatsapp && !digest[0].email);
        assert!(format!("{}{}", digest[0].title, digest[0].body).contains("Call them back"));
        notify::discard_outbox();

        followups::fire_due(&mut c, None).await.unwrap(); // nothing fires twice
        assert_eq!(bells(&mut c, &rid_s).await.len(), 1);
        assert!(notify::pending_outbox()
            .iter()
            .all(|i| i.user_ids != vec![owner.id.to_string()]));

        // still open a day later: email
        sqlx::query("UPDATE crm_reminders SET notified_at = $1 WHERE id = $2")
            .bind(now() - TimeDelta::hours(30))
            .bind(rid)
            .execute(&mut *c)
            .await
            .unwrap();
        followups::fire_due(&mut c, None).await.unwrap();
        assert_eq!(bells(&mut c, &rid_s).await.len(), 2);
        let esc: Vec<_> = notify::pending_outbox()
            .into_iter()
            .filter(|i| i.user_ids == vec![owner.id.to_string()])
            .collect();
        assert_eq!(esc.len(), 1);
        assert!(esc[0].email && esc[0].title.starts_with("Still open"));
        notify::discard_outbox();
    })
    .await;
}

#[tokio::test]
async fn test_nothing_is_delivered_outside_the_window() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let owner = make_person(&mut c, false, &[]).await;
    let rid = make(&mut c, owner.id, now() - TimeDelta::minutes(5)).await;
    set_setting(&mut c, "crm.followup.quiet_from", "23:58").await;
    set_setting(&mut c, "crm.followup.quiet_to", "23:59").await;
    let res = notify::scope(async {
        (
            followups::fire_due(&mut c, None).await.unwrap(),
            notify::pending_outbox(),
        )
    })
    .await;
    base_settings(&mut c).await;
    // (unless the test happens to run inside 23:58-23:59 company time)
    assert_eq!(res.0.quiet, 1);
    assert!(res.1.is_empty());
    assert!(bells(&mut c, &rid.hyphenated().to_string())
        .await
        .is_empty());
}

#[tokio::test]
async fn test_weekly_digest_lists_overdue_by_owner() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    base_settings(&mut c).await;
    let _top = make_person(&mut c, true, &[]).await;
    let owner = make_person(&mut c, false, &[]).await;
    make(&mut c, owner.id, now() - TimeDelta::days(2)).await;
    notify::scope(async {
        let total = followups::weekly_digest(&mut c, None).await.unwrap();
        assert!(total >= 1);
        let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
            "SELECT user_id, body FROM notifications WHERE entity_type = 'crm_reminder_digest'",
        )
        .fetch_all(&mut *c)
        .await
        .unwrap();
        // Python's test DB has no employee positioned Owner, so every superadmin (the new one too) is "the Owner";
        // the seeded database may have one: the digest goes to whoever owner_user_ids names.
        let owners = granary_services::approvals::routing::owner_user_ids(&mut c)
            .await
            .unwrap();
        let _ = &owner;
        assert!(!owners.is_empty());
        for id in &owners {
            assert!(rows.iter().any(|(uid, body)| uid.to_string() == *id
                && body.as_deref().unwrap_or("").contains("overdue")));
        }
        assert!(notify::pending_outbox()
            .iter()
            .any(|i| i.email && i.user_ids == owners));
        notify::discard_outbox();
    })
    .await;
}

// ─── Other files' follow-up tests ────────────────────────────────────────────

/// test_nary_p0_promote.py
#[tokio::test]
async fn test_a_due_next_action_becomes_a_follow_up() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let owner = make_person(&mut c, true, &[]).await;
    let lead_id = lead(&mut c, &u("P0 lead"), Some(owner.id), None, 0).await;
    let yesterday = granary_core::clock::today_local(&mut c).await.unwrap() - TimeDelta::days(1);
    let mut ids = vec![];
    for (next, done) in [("Call back about the pilot", false), ("Already done", true)] {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO activities (id, entity_type, entity_id, lead_id, activity_type, next_action, is_completed, \
             created_by_id, next_action_due, direction) VALUES ($1, 'lead', $2, $2, 'call', $3, $4, $5, $6, 'outbound')",
        )
        .bind(id)
        .bind(lead_id)
        .bind(next)
        .bind(done)
        .bind(owner.id)
        .bind(yesterday)
        .execute(&mut *c)
        .await
        .unwrap();
        ids.push(id);
    }
    let cfg = followups::settings(&mut c).await.unwrap();
    followups::rule_next_action(&mut c, &cfg).await.unwrap();
    let rows: Vec<(String, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT title, owner_user_id, rule_key FROM crm_reminders WHERE entity_id = $1 AND source = 'next_action'",
    )
    .bind(lead_id)
    .fetch_all(&mut *c)
    .await
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        vec!["Next action due: Call back about the pilot"]
    );
    assert_eq!(rows[0].1, owner.id);
    let key = rows[0].2.clone().unwrap();
    assert!(key.contains(&ids[0].to_string()) && !key.contains(&ids[1].to_string()));
}

/// test_nary_p0_stages.py
#[tokio::test]
async fn test_the_stale_deal_rule_counts_time_in_stage() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let owner = make_person(&mut c, true, &[]).await;
    let long_ago = now() - TimeDelta::days(60);
    let fresh = deal(&mut c, "new", Some(owner.id), long_ago).await;
    let stuck = deal(&mut c, "new", Some(owner.id), long_ago).await;
    let won = deal(&mut c, "won", Some(owner.id), long_ago).await;
    sqlx::query("UPDATE deals SET stage = 'proposal' WHERE id = ANY($1)")
        .bind(vec![fresh, stuck])
        .execute(&mut *c)
        .await
        .unwrap();
    for (id, from, at) in [
        (fresh, "contract", now() - TimeDelta::days(1)),
        (stuck, "discovery", long_ago),
    ] {
        sqlx::query("INSERT INTO deal_stage_events (id, deal_id, from_stage, to_stage, at, origin) VALUES ($1, $2, $3, 'proposal', $4, 'rest')")
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(from)
            .bind(at)
            .execute(&mut *c)
            .await
            .unwrap();
    }
    let cfg = followups::settings(&mut c).await.unwrap();
    followups::rule_stale_deal(&mut c, &cfg).await.unwrap();
    let count = |id: Uuid| {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM crm_reminders WHERE entity_id = $1 AND source = 'stale_deal'",
        )
        .bind(id)
    };
    assert_eq!(count(stuck).fetch_one(&mut *c).await.unwrap(), 1);
    assert_eq!(count(fresh).fetch_one(&mut *c).await.unwrap(), 0);
    assert_eq!(count(won).fetch_one(&mut *c).await.unwrap(), 0);
}

/// test_nary_chat_fixes.py
#[tokio::test]
async fn test_a_never_touched_lead_gets_one_reminder_not_one_a_day() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let owner = make_person(&mut c, false, &[]).await;
    let lead_id = lead(&mut c, &u("Pytest quiet"), Some(owner.id), None, 40).await;
    let cfg = FollowupSettings {
        max_per_rule_pass: 100000,
        ..Default::default()
    };
    followups::rule_stale_lead(&mut c, &cfg).await.unwrap();
    followups::set_now_override(Some(now() + TimeDelta::days(1))); // the next day
    followups::rule_stale_lead(&mut c, &cfg).await.unwrap();
    followups::set_now_override(None);
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT rule_key FROM crm_reminders WHERE entity_id = $1 AND status = 'open'",
    )
    .bind(lead_id)
    .fetch_all(&mut *c)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0.as_deref(),
        Some(format!("stale_lead:{lead_id}").as_str())
    );
}

/// test_nary_chat_fixes.py
#[tokio::test]
async fn test_whatsapp_traffic_on_a_lead_keeps_it_from_looking_quiet() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let owner = make_person(&mut c, false, &[]).await;
    let lead_id = lead(&mut c, &u("Pytest quiet"), Some(owner.id), None, 40).await;
    let cid = format!("62{}@c.us", &Uuid::new_v4().as_u128().to_string()[..10]);
    sqlx::query("INSERT INTO wa_chats (id, chat_id, status, is_my_contact) VALUES ($1, $2, 'active', false)")
        .bind(Uuid::new_v4())
        .bind(&cid)
        .execute(&mut *c)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, type, body, wa_ts, lead_id, origin, has_media) \
         VALUES ($1, $2, $3, 'inbound', false, 'chat', 'Hello', $4, $5, 'live', false)",
    )
    .bind(Uuid::new_v4())
    .bind(&Uuid::new_v4().simple().to_string()[..20])
    .bind(&cid)
    .bind(now() - TimeDelta::days(1))
    .bind(lead_id)
    .execute(&mut *c)
    .await
    .unwrap();
    followups::rule_stale_lead(
        &mut c,
        &FollowupSettings {
            max_per_rule_pass: 100000,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM crm_reminders WHERE entity_id = $1")
        .bind(lead_id)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

/// test_nary_chat_fixes.py
#[tokio::test]
async fn test_follow_ups_due_together_make_one_digest() {
    let d = db(TAG).await; // (settings only)
    drop(d);
    let items: Vec<_> = (0..13)
        .map(|i| {
            serde_json::from_value::<granary_db::rows::crm::CrmReminder>(serde_json::json!({
                "owner_user_id": Uuid::new_v4(), "source": "stale_lead", "entity_type": "lead",
                "entity_id": Uuid::new_v4(), "title": format!("Lead has gone quiet: {i}"),
                "note": "Nothing has happened on it yet.", "due_at": "2026-01-01T00:00:00Z", "status": "open",
                "snooze_count": 0, "id": Uuid::new_v4(), "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap()
        })
        .collect();
    let out = followups::digest_message(&items, 10);
    assert_eq!(out.title, "Follow-ups due (13)");
    assert!(out.body.contains("10. Lead has gone quiet: 9"));
    assert!(out.body.ends_with("+3 more") && out.body.contains("/crm/leads/"));
    let _ = Utc::now().year();
}
