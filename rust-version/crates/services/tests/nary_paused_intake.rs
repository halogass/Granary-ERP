//! 29 Oct 2026 (Owner decisions 27-29 Oct): intake and quiet CRM bookkeeping keep working while Nary is paused.
//!
//! - A website submission while paused becomes a CRM lead (status processed), its lead.created bookkeeping runs,
//!   the Owner's website lead alert is raised, and nothing goes to the visitor: no email, no card. On resume the
//!   acknowledgement goes once and the intake does not run again.
//! - A new email to the CRM inbox while paused becomes a lead; the acknowledgement, the reply playbook and the
//!   promotion card wait for the resume.
//! - Duplicate-deal detection runs while paused (the newer deal is held).
//! - Nary's own subscribers wait: a message.inbound stays undispatched while paused. On resume an event older than
//!   nary.events.resume_max_age_hours is marked skipped without running them, a fresh one runs; quiet subscribers
//!   are not run twice; an event the paused dispatcher never saw runs everything whatever its age.
//!
//! The pipeline runs on the brain seam (ai::registry::seam), the email seams and the real dispatcher with every
//! module's subscribers. Process-wide pools, hooks and seams: one test at a time (SERIAL and the seam lock).

mod nary_n6_common;
mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

use granary_services::ai::registry::seam::{Brain, Seam};
use granary_services::crm_inbox::{self, NaryCalls};
use granary_services::email::{self as email_service, Seams, SendCall, SmtpConfig};
use granary_services::imap_sync::pyemail::Message;
use granary_services::nary::{self, email_intake, events, intake, web3forms};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const MAILBOX: &str = "hello@example.com";

struct Env {
    pool: PgPool,
    _brain: Brain,
    _seam: Seam,
    sent: Arc<Mutex<Vec<SendCall>>>,
}

fn install_hooks() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        crm_inbox::set_nary_calls(NaryCalls {
            header_auth: Some(Arc::new(|m: &Message| web3forms::header_auth(m))),
            detect_and_queue: Some(Arc::new(|db, msg, thread| {
                Box::pin(intake::detect_and_queue(db, msg, thread))
            })),
            email_intake_queue: Some(Arc::new(|db, thread, msg, headers, mailbox| {
                Box::pin(email_intake::queue(db, thread, msg, headers, mailbox))
            })),
            emit: Some(Arc::new(|db, t, et, eid, payload| {
                Box::pin(async move {
                    events::emit(db, t, et, eid, payload, None).await?;
                    Ok(())
                })
            })),
        });
        granary_services::approvals::notify::set_external_delivery_enabled(false);
    });
}

async fn setup(tag: &str) -> Env {
    let brain = Brain::p2();
    let seam = brain.install().await;
    let pool = support::fresh_db(tag).await;
    nary::set_session_pool(pool.clone());
    crm_inbox::set_session_pool(pool.clone());
    granary_services::ai::registry::set_session_pool(pool.clone());
    install_hooks();
    let sent: Arc<Mutex<Vec<SendCall>>> = Arc::new(Mutex::new(vec![]));
    let rec = sent.clone();
    email_service::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call: SendCall| {
            rec.lock().unwrap().push(call);
            Box::pin(async { Ok((true, None)) })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: "u".into(),
                    password: "p".into(),
                    from_name: "Your Company Ltd".into(),
                    from_email: MAILBOX.into(),
                    tls: true,
                })
            })
        })),
    }));
    let mut c = pool.acquire().await.unwrap();
    nary_n6_common::working_hours(&mut c, true).await;
    nary_n6_common::owner(&mut c).await;
    Env {
        pool,
        _brain: brain,
        _seam: seam,
        sent,
    }
}

async fn set(pool: &PgPool, k: &str, v: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(k)
        .bind(v)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(n.rows_affected(), 1, "setting not seeded: {k}");
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Runs the dispatcher until it has nothing left; the summed counters.
async fn dispatch(pool: &PgPool) -> Value {
    let mut sum = json!({"dispatched": 0, "quiet": 0, "failed": 0, "skipped": 0});
    for _ in 0..20 {
        let out = events::dispatch_pending(Some(pool)).await.unwrap();
        let mut any = false;
        for k in ["dispatched", "quiet", "failed", "skipped"] {
            let n = out[k].as_i64().unwrap_or(0);
            any |= n > 0;
            sum[k] = json!(sum[k].as_i64().unwrap() + n);
        }
        if !any {
            break;
        }
    }
    sum
}

/// (quiet_done_at is set, dispatched_at is set, last_error) of each event of a type, oldest first.
async fn event_state(pool: &PgPool, t: &str) -> Vec<(bool, bool, Option<String>)> {
    sqlx::query_as(
        "SELECT quiet_done_at IS NOT NULL, dispatched_at IS NOT NULL, last_error FROM domain_events \
         WHERE type = $1 ORDER BY occurred_at",
    )
    .bind(t)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn raw_email(sender: &str, body: &str, subject: &str) -> Vec<u8> {
    let mid = format!("<{}@pytest.local>", Uuid::new_v4().simple());
    let mut s = format!(
        "From: Rachel Wilson <{sender}>\r\nTo: {MAILBOX}\r\nSubject: {subject}\r\nMessage-ID: {mid}\r\nDate: {}\r\n",
        Utc::now().format("%a, %d %b %Y %H:%M:%S +0000")
    );
    s.push_str("Content-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: 8bit\r\nMIME-Version: 1.0\r\n\r\n");
    s.push_str(&body.replace('\n', "\r\n"));
    s.push_str("\r\n");
    s.into_bytes()
}

#[tokio::test]
async fn a_new_email_becomes_a_lead_while_paused_and_is_answered_on_resume() {
    let _serial = SERIAL.lock().await;
    let env = setup("n9_paused_email").await;
    let pool = &env.pool;
    set(pool, "nary.email.intake_enabled", "true").await;
    set(pool, "nary.ack.enabled", "true").await;
    set(pool, "nary.paused", "true").await;
    let addr = format!(
        "rachel.{}@vandelay-pytest.example",
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let raw = raw_email(
        &addr,
        "Good afternoon, we need laptops for 12 staff at our warehouse.\n\nRina",
        "Laptop question",
    );
    {
        let mut c = pool.acquire().await.unwrap();
        let mut tx = c.begin().await.unwrap();
        assert!(
            crm_inbox::_ingest(&mut tx, &raw, 7, "INBOX", false, MAILBOX)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
    }
    let out = dispatch(pool).await;
    assert_eq!(out["failed"], 0, "{out}");
    let (thread_id, lead_id): (Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT t.id, t.linked_lead_id FROM crm_email_threads t JOIN crm_email_messages m ON m.thread_id = t.id \
         WHERE m.from_email = $1",
    )
    .bind(&addr)
    .fetch_one(pool)
    .await
    .unwrap();
    let lead_id = lead_id.expect("a lead while paused");
    let source: String = sqlx::query_scalar("SELECT source FROM leads WHERE id = $1")
        .bind(lead_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(source, "email");
    assert!(env.sent.lock().unwrap().is_empty(), "nothing sent");
    assert_eq!(
        count(pool, "SELECT count(*) FROM agent_pending_actions").await,
        0,
        "no card while paused"
    );
    assert_eq!(
        event_state(pool, "email.new").await,
        vec![(true, false, None)]
    );

    set(pool, "nary.paused", "false").await;
    let out = dispatch(pool).await;
    assert_eq!(out["failed"], 0, "{out}");
    let sent = env.sent.lock().unwrap().clone();
    assert!(
        sent.len() == 1 && sent[0].to == vec![addr.clone()],
        "the acknowledgement once: {sent:?}"
    );
    let reply: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM agent_pending_actions WHERE action = 'email.reply.send'",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(reply.len(), 1, "the reply playbook ran on resume");
    let promote = count(
        pool,
        "SELECT count(*) FROM agent_pending_actions WHERE action = 'lead.promote'",
    )
    .await;
    assert_eq!(promote, 1);
    // the intake did not run twice: one lead for the thread
    let leads = count(
        pool,
        &format!("SELECT count(*) FROM leads l JOIN contacts c ON c.id = l.contact_id WHERE c.email = '{addr}'"),
    )
    .await;
    assert_eq!(leads, 1);
    let linked: Option<Uuid> =
        sqlx::query_scalar("SELECT linked_lead_id FROM crm_email_threads WHERE id = $1")
            .bind(thread_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(linked, Some(lead_id));
}

static NARY_RAN: AtomicUsize = AtomicUsize::new(0);
static QUIET_RAN: AtomicUsize = AtomicUsize::new(0);

async fn emit(pool: &PgPool, t: &str, entity: Uuid, age_hours: i64) -> Uuid {
    let mut tx = pool.begin().await.unwrap();
    let id = events::emit(&mut tx, t, Some("deal"), Some(entity), None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE domain_events SET occurred_at = $2 WHERE id = $1")
        .bind(id)
        .bind(Utc::now() - Duration::hours(age_hours))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn nary_own_work_waits_and_old_events_are_skipped_on_resume() {
    let _serial = SERIAL.lock().await;
    let env = setup("n9_paused_resume").await;
    let pool = &env.pool;
    // a stand-in next to channels.rs (Nary's work) and a quiet one, both on message.inbound
    let nary_id = events::subscribe(
        &["message.inbound"],
        events::subscriber(|_db, _ev| {
            Box::pin(async {
                NARY_RAN.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }),
    );
    let quiet_id = events::subscribe_quiet(
        &["message.inbound"],
        events::subscriber(|_db, _ev| {
            Box::pin(async {
                QUIET_RAN.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }),
    );
    NARY_RAN.store(0, Ordering::SeqCst);
    QUIET_RAN.store(0, Ordering::SeqCst);

    // duplicate-deal detection runs while paused: the newer deal of a pair is held
    let mut c = pool.acquire().await.unwrap();
    let cust = nary_n6_common::customer(&mut c, None, false, "en").await;
    let older = nary_n6_common::deal_for(&mut c, &cust, "qualified").await;
    let newer = nary_n6_common::deal_for(&mut c, &cust, "qualified").await;
    sqlx::query("UPDATE deals SET created_at = now() - interval '1 day' WHERE id = $1")
        .bind(older)
        .execute(&mut *c)
        .await
        .unwrap();

    set(pool, "nary.paused", "true").await;
    let old = emit(pool, "message.inbound", Uuid::new_v4(), 30).await;
    let fresh = emit(pool, "message.inbound", Uuid::new_v4(), 1).await;
    emit(pool, "deal.created", newer, 0).await;
    let out = dispatch(pool).await;
    assert_eq!(out["failed"], 0, "{out}");
    assert_eq!(out["quiet"], 3, "{out}");
    assert_eq!(QUIET_RAN.load(Ordering::SeqCst), 2);
    assert_eq!(NARY_RAN.load(Ordering::SeqCst), 0, "Nary's own work waits");
    assert_eq!(
        event_state(pool, "message.inbound").await,
        vec![(true, false, None), (true, false, None)],
        "message.inbound stays undispatched while paused"
    );
    let held: Option<String> =
        sqlx::query_scalar("SELECT custom ->> 'duplicate_of' FROM deals WHERE id = $1")
            .bind(newer)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(held, Some(older.to_string()), "the duplicate check ran");
    assert_eq!(
        count(pool, "SELECT count(*) FROM agent_pending_actions").await,
        0,
        "no card while paused"
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM agent_runs").await,
        0,
        "no Nary run while paused"
    );

    // resume: the old one is skipped, the fresh one runs Nary's work only; the quiet one is not run again
    set(pool, "nary.paused", "false").await;
    // an event the paused dispatcher never saw runs everything, whatever its age
    let unseen = emit(pool, "message.inbound", Uuid::new_v4(), 48).await;
    let out = dispatch(pool).await;
    assert_eq!(out["failed"], 0, "{out}");
    assert_eq!(out["skipped"], 1, "{out}");
    assert_eq!(
        NARY_RAN.load(Ordering::SeqCst),
        2,
        "the fresh one and the unseen one"
    );
    assert_eq!(
        QUIET_RAN.load(Ordering::SeqCst),
        3,
        "quiet subscribers run once per event: only the unseen one adds"
    );
    let row = |id: Uuid| async move {
        sqlx::query_as::<_, (bool, Option<String>)>(
            "SELECT dispatched_at IS NOT NULL, last_error FROM domain_events WHERE id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    };
    assert_eq!(
        row(old).await,
        (
            true,
            Some("skipped: arrived while Nary was paused, older than 24 h".to_string())
        )
    );
    assert_eq!(row(fresh).await, (true, None));
    assert_eq!(row(unseen).await, (true, None));
    // nothing pending, nothing runs twice
    let out = dispatch(pool).await;
    assert_eq!(
        out,
        json!({"dispatched": 0, "quiet": 0, "failed": 0, "skipped": 0})
    );
    assert_eq!(NARY_RAN.load(Ordering::SeqCst), 2);

    // a missing setting is a clear error naming it
    sqlx::query("UPDATE domain_events SET dispatched_at = NULL WHERE id = $1")
        .bind(old)
        .execute(pool)
        .await
        .unwrap();
    let saved: Option<String> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'nary.events.resume_max_age_hours'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM system_settings WHERE key = 'nary.events.resume_max_age_hours'")
        .execute(pool)
        .await
        .unwrap();
    let err = events::dispatch_pending(Some(pool)).await.unwrap_err();
    assert!(
        err.to_string().contains("nary.events.resume_max_age_hours"),
        "{err}"
    );
    sqlx::query(
        "INSERT INTO system_settings (id, created_at, updated_at, key, value, value_type, category, label, is_public, \
         is_encrypted) VALUES (gen_random_uuid(), now(), now(), 'nary.events.resume_max_age_hours', $1, 'int', 'nary', \
         'x', false, false)",
    )
    .bind(saved)
    .execute(pool)
    .await
    .unwrap();

    events::unsubscribe(nary_id);
    events::unsubscribe(quiet_id);
}
