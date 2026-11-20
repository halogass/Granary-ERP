//! W5-F's example tasks: poll_crm_inbox, send_scheduled_messages (Python test_audit_critical.py
//! `test_a_scheduled_message_is_claimed_once`, plus the send paths that have no Python test),
//! nary_dispatch_events, nary_staff_chat_reply (busy -> retry with countdown 3, waited = retries * 3).

mod support;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use futures::StreamExt;
use serde_json::{json, Map, Value};
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use granary_worker::registry::Outcome;
use support::{env, TestEnv};

const TAG: &str = "examples";
const REDIS_DB: u32 = 15;

// ─── Fake WhatsApp service ───────────────────────────────────────────────────

/// What the fake wwebjs received: (Authorization header, JSON body).
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

struct Wa {
    seen: Seen,
}

impl Respond for Wa {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let auth = req
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        self.seen.lock().unwrap().push((auth, body.clone()));
        let to = body["to"].as_str().unwrap_or("").to_string();
        if to.starts_with("fail") {
            return ResponseTemplate::new(500).set_body_string("boom");
        }
        let key = format!("KEY{}", &Uuid::new_v4().simple().to_string()[..12]);
        ResponseTemplate::new(200)
            .set_body_json(json!({"messageId": format!("true_{to}_{key}"), "chatId": to}))
    }
}

static WA: OnceLock<(String, Seen)> = OnceLock::new();

/// The env, with WHATSAPP_SERVICE_URL pointing at the fake (started once, before the settings load).
fn the_env() -> &'static TestEnv {
    WA.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let server = MockServer::start().await;
                let seen: Seen = Arc::default();
                Mock::given(wiremock::matchers::method("POST"))
                    .and(wiremock::matchers::path("/send"))
                    .respond_with(Wa { seen: seen.clone() })
                    .mount(&server)
                    .await;
                tx.send((server.uri(), seen)).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let (uri, seen) = rx.recv().unwrap();
        std::env::set_var("WHATSAPP_SERVICE_URL", &uri);
        (uri, seen)
    });
    env(TAG, REDIS_DB)
}

fn wa_seen() -> Vec<(String, Value)> {
    WA.get().unwrap().1.lock().unwrap().clone()
}

// ─── Rows ────────────────────────────────────────────────────────────────────

async fn user(e: &TestEnv, name: &str, superadmin: bool) -> Uuid {
    let mut db = e.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, $4, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@example.test", support::u("pytest.tasks")))
    .bind(name)
    .bind(superadmin)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

/// Gives `user` the permission codes through a role of its own.
async fn grant(e: &TestEnv, user: Uuid, codes: &[&str]) {
    let mut db = e.db().await;
    let role = Uuid::new_v4();
    let slug = support::u("role");
    sqlx::query("INSERT INTO roles (id, name, slug) VALUES ($1, $2, $2)")
        .bind(role)
        .bind(&slug)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
        .bind(user)
        .bind(role)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) SELECT $1, id FROM permissions WHERE code = ANY($2)",
    )
    .bind(role)
    .bind(codes)
    .execute(&mut db)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn scheduled(
    e: &TestEnv,
    channel: &str,
    owner: Uuid,
    chat_id: &str,
    recipient: &str,
    subject: Option<&str>,
    status: &str,
    retry_count: i32,
) -> Uuid {
    let mut db = e.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO scheduled_messages (id, channel, created_by_id, chat_id, recipient, subject, body, scheduled_at, \
         status, retry_count) VALUES ($1, $2, $3, $4, $5, $6, 'Reminder: invoice due tomorrow', $7, $8, $9)",
    )
    .bind(id)
    .bind(channel)
    .bind(owner)
    .bind(chat_id)
    .bind(recipient)
    .bind(subject)
    .bind(Utc::now() - TimeDelta::minutes(1))
    .bind(status)
    .bind(retry_count)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

#[derive(Debug, sqlx::FromRow)]
struct Row {
    status: Option<String>,
    retry_count: Option<i32>,
    last_error: Option<String>,
    sent_at: Option<chrono::DateTime<Utc>>,
}

async fn row(e: &TestEnv, id: Uuid) -> Row {
    sqlx::query_as(
        "SELECT status, retry_count, last_error, sent_at FROM scheduled_messages WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut e.db().await)
    .await
    .unwrap()
}

async fn notifications(e: &TestEnv, user: Uuid) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT type, body, priority, entity_id FROM notifications WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user)
    .fetch_all(&mut e.db().await)
    .await
    .unwrap()
}

async fn cleanup(e: &TestEnv, ids: &[Uuid]) {
    sqlx::query("DELETE FROM scheduled_messages WHERE id = ANY($1)")
        .bind(ids)
        .execute(&mut e.db().await)
        .await
        .unwrap();
}

async fn send_scheduled(e: &TestEnv) -> Value {
    match e
        .call("app.tasks.send_scheduled_messages", vec![], 0)
        .await
        .unwrap()
    {
        Outcome::Done(v) => v,
        other => panic!("{other:?}"),
    }
}

// ─── send_scheduled_messages ─────────────────────────────────────────────────

#[tokio::test]
async fn test_a_scheduled_message_is_claimed_once() {
    let e = the_env();
    let _g = e.serial().await; // send_scheduled_messages tests would pick the pending row up
    let owner = user(e, "Claim Owner", false).await;
    let msg_id = scheduled(
        e,
        "email",
        owner,
        &Uuid::new_v4().to_string(),
        "x@example.com",
        None,
        "pending",
        0,
    )
    .await;
    let state = e.state.clone();
    let (first, second) = e
        .run(async move {
            let db = granary_core::db::DbSession::new(state.db.clone());
            let first = granary_worker::tasks::crm::_claim_scheduled_message(&db, msg_id)
                .await
                .unwrap();
            let db = granary_core::db::DbSession::new(state.db.clone()); // an overlapping run gets nothing
            let second = granary_worker::tasks::crm::_claim_scheduled_message(&db, msg_id)
                .await
                .unwrap();
            (first, second)
        })
        .await;
    assert!(first);
    assert!(!second);
    assert_eq!(row(e, msg_id).await.status.as_deref(), Some("sending"));
    cleanup(e, &[msg_id]).await;
}

#[tokio::test]
async fn a_due_whatsapp_message_is_signed_sent_logged_and_broadcast() {
    let e = the_env();
    let _g = e.serial().await;
    let author = user(e, "Rachel Amy", true).await;
    let chat = format!("81901{}@c.us", rand_digits());
    let id = scheduled(e, "whatsapp", author, &chat, "81901", None, "pending", 0).await;
    let before = wa_seen().len();

    let client = redis::Client::open(e.redis_url.as_str()).unwrap();
    let mut ps = client.get_async_pubsub().await.unwrap();
    ps.subscribe("whatsapp:broadcast").await.unwrap();
    let out = send_scheduled(e).await;
    let mut published = vec![];
    {
        let mut stream = ps.on_message();
        while let Ok(Some(m)) =
            tokio::time::timeout(Duration::from_millis(500), stream.next()).await
        {
            published.push(m.get_payload::<String>().unwrap());
        }
    }

    assert_eq!(
        out,
        json!("Scheduled messages: 1 sent, 0 permanently failed")
    );
    let r = row(e, id).await;
    assert_eq!(r.status.as_deref(), Some("sent"));
    assert!(r.sent_at.is_some());
    let seen = wa_seen();
    assert_eq!(seen.len(), before + 1);
    let (auth, body) = &seen[before];
    assert_eq!(auth, "Bearer test-wa-token");
    assert_eq!(body["to"], json!(chat));
    let message = body["message"].as_str().unwrap();
    assert!(
        message.starts_with("Reminder: invoice due tomorrow\n\n_\u{2014} Rachel A., "),
        "{message}"
    );
    // "sent by" is logged under the message key.
    let logged: Vec<(String, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT message_key, user_id, sender_name FROM wa_sent_messages WHERE chat_id = $1",
    )
    .bind(&chat)
    .fetch_all(&mut e.db().await)
    .await
    .unwrap();
    assert_eq!(logged.len(), 1);
    assert!(logged[0].0.starts_with("KEY"));
    assert_eq!(logged[0].1, Some(author));
    assert_eq!(logged[0].2, "Rachel A.");
    assert_eq!(
        published,
        vec![format!(
            "{{\"type\": \"scheduled_message_sent\", \"chatId\": \"{chat}\", \"scheduledMessageId\": \"{id}\"}}"
        )]
    );
    let n = notifications(e, author).await;
    assert_eq!(
        n,
        vec![(
            "scheduled_message_sent".to_string(),
            "Your scheduled whatsapp message to 81901 was delivered.".to_string(),
            "normal".to_string(),
            id.to_string()
        )]
    );
    cleanup(e, &[id]).await;
}

/// Batch E: the author's right is checked again at send time; without it the message fails at once, with
/// the reason (retrying cannot help), and the author is told why.
#[tokio::test]
async fn an_author_without_the_send_right_fails_at_once_with_the_reason() {
    let e = the_env();
    let _g = e.serial().await;
    let author = user(e, "No Rights", false).await;
    let chat = format!("8190{}@c.us", rand_digits());
    let id = scheduled(e, "whatsapp", author, &chat, "81902", None, "pending", 0).await;
    let before = wa_seen().len();
    assert_eq!(
        send_scheduled(e).await,
        json!("Scheduled messages: 0 sent, 1 permanently failed")
    );
    let r = row(e, id).await;
    assert_eq!(r.status.as_deref(), Some("failed"));
    assert_eq!(r.retry_count, Some(1));
    assert_eq!(
        r.last_error.as_deref(),
        Some("The author no longer has permission to send WhatsApp messages")
    );
    assert_eq!(wa_seen().len(), before, "nothing was sent");
    assert_eq!(
        notifications(e, author).await,
        vec![(
            "scheduled_message_failed".to_string(),
            "Your scheduled whatsapp message to 81902 was not sent: The author no longer has permission to send WhatsApp messages."
                .to_string(),
            "high".to_string(),
            id.to_string()
        )]
    );
    cleanup(e, &[id]).await;
}

/// Batch E: send rights without inbox access (crm:view_whatsapp) are not enough, as at scheduling time; a
/// deactivated author's messages fail too, and an email author needs crm:send_email.
#[tokio::test]
async fn the_send_time_check_covers_inbox_access_active_status_and_email() {
    let e = the_env();
    let _g = e.serial().await;
    let send_only = user(e, "Send Only", false).await;
    grant(e, send_only, &["crm:send_whatsapp"]).await;
    let gone = user(e, "Gone Author", false).await;
    grant(e, gone, &["crm:view_whatsapp", "crm:send_whatsapp"]).await;
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(gone)
        .execute(&mut e.db().await)
        .await
        .unwrap();
    let mailer = user(e, "No Mail Right", false).await;
    let a = scheduled(
        e,
        "whatsapp",
        send_only,
        &format!("8170{}@c.us", rand_digits()),
        "81701",
        None,
        "pending",
        0,
    )
    .await;
    let b = scheduled(
        e,
        "whatsapp",
        gone,
        &format!("8171{}@c.us", rand_digits()),
        "81702",
        None,
        "pending",
        0,
    )
    .await;
    let c = scheduled(
        e,
        "email",
        mailer,
        &Uuid::new_v4().to_string(),
        "x@example.com",
        None,
        "pending",
        0,
    )
    .await;
    let before = wa_seen().len();
    assert_eq!(
        send_scheduled(e).await,
        json!("Scheduled messages: 0 sent, 3 permanently failed")
    );
    assert_eq!(wa_seen().len(), before);
    let ra = row(e, a).await;
    assert_eq!(ra.status.as_deref(), Some("failed"));
    assert_eq!(
        ra.last_error.as_deref(),
        Some("The author no longer has permission to send WhatsApp messages")
    );
    let rb = row(e, b).await;
    assert_eq!(rb.status.as_deref(), Some("failed"));
    assert_eq!(
        rb.last_error.as_deref(),
        Some("The author's account is deactivated")
    );
    let rc = row(e, c).await;
    assert_eq!(rc.status.as_deref(), Some("failed"));
    assert_eq!(
        rc.last_error.as_deref(),
        Some("The author no longer has permission to send email")
    );
    cleanup(e, &[a, b, c]).await;
}

#[tokio::test]
async fn a_wwebjs_error_is_recorded_and_retried() {
    let e = the_env();
    let _g = e.serial().await;
    let author = user(e, "Super Sender", true).await;
    let chat = format!("fail{}@c.us", rand_digits());
    let id = scheduled(e, "whatsapp", author, &chat, "fail", None, "pending", 0).await;
    send_scheduled(e).await;
    let r = row(e, id).await;
    assert_eq!(r.status.as_deref(), Some("pending"));
    assert_eq!(r.retry_count, Some(1));
    assert_eq!(r.last_error.as_deref(), Some("wwebjs returned 500: boom"));
    // crm.scheduled.max_attempts is 12 (seed): the 12th failure is final.
    sqlx::query("UPDATE scheduled_messages SET retry_count = 11 WHERE id = $1")
        .bind(id)
        .execute(&mut e.db().await)
        .await
        .unwrap();
    assert_eq!(
        send_scheduled(e).await,
        json!("Scheduled messages: 0 sent, 1 permanently failed")
    );
    let r = row(e, id).await;
    assert_eq!(r.status.as_deref(), Some("failed"));
    assert_eq!(r.retry_count, Some(12));
    assert_eq!(
        notifications(e, author).await,
        vec![(
            "scheduled_message_failed".to_string(),
            "Your scheduled whatsapp message to fail could not be delivered after multiple retries."
                .to_string(),
            "high".to_string(),
            id.to_string()
        )]
    );
    cleanup(e, &[id]).await;
}

#[tokio::test]
async fn a_send_stuck_in_sending_is_failed_not_resent() {
    let e = the_env();
    let _g = e.serial().await;
    let author = user(e, "Stuck Owner", true).await;
    let chat = format!("8180{}@c.us", rand_digits());
    let id = scheduled(e, "whatsapp", author, &chat, "81903", None, "sending", 0).await;
    sqlx::query(
        "UPDATE scheduled_messages SET updated_at = now() - interval '31 minutes' WHERE id = $1",
    )
    .bind(id)
    .execute(&mut e.db().await)
    .await
    .unwrap();
    let before = wa_seen().len();
    send_scheduled(e).await;
    let r = row(e, id).await;
    assert_eq!(r.status.as_deref(), Some("failed"));
    assert_eq!(
        r.last_error.as_deref(),
        Some("Interrupted while sending; check the chat before resending")
    );
    assert_eq!(wa_seen().len(), before);
    cleanup(e, &[id]).await;
}

#[tokio::test]
async fn a_due_email_goes_out_through_crm_smtp_and_is_stored_in_its_thread() {
    use granary_services::email::{Seams, SendCall, SmtpConfig};
    let e = the_env();
    let _g = e.serial().await;
    let author = user(e, "Mail Author", false).await;
    grant(e, author, &["crm:send_email"]).await;
    // the seed ships no CRM inbox: the one the crm_smtp_* settings belong to (prefix "crm")
    sqlx::query(
        "INSERT INTO crm_mailboxes (id, prefix, address, display_name, purpose, active, is_default, sort) \
         SELECT gen_random_uuid(), 'crm', 'hello@example.test', 'Hello', 'general', true, true, 0 \
         WHERE NOT EXISTS (SELECT 1 FROM crm_mailboxes WHERE prefix = 'crm')",
    )
    .execute(&mut e.db().await)
    .await
    .unwrap();
    let thread = Uuid::new_v4();
    sqlx::query("INSERT INTO crm_email_threads (id, subject) VALUES ($1, 'Order follow-up')")
        .bind(thread)
        .execute(&mut e.db().await)
        .await
        .unwrap();
    let ok = scheduled(
        e,
        "email",
        author,
        &thread.to_string(),
        "client@example.com",
        None,
        "pending",
        0,
    )
    .await;
    let calls: Arc<Mutex<Vec<SendCall>>> = Arc::default();
    let c2 = calls.clone();
    let host = Arc::new(Mutex::new("smtp.test".to_string()));
    let h2 = host.clone();
    granary_services::email::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call| {
            c2.lock().unwrap().push(call);
            Box::pin(async { Ok((true, None)) })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(move || {
            let host = h2.lock().unwrap().clone();
            Box::pin(async move {
                Ok(SmtpConfig {
                    host,
                    port: 587,
                    user: String::new(),
                    password: String::new(),
                    from_name: "Your Company".into(),
                    from_email: "hello@example.test".into(),
                    tls: false,
                })
            })
        })),
    }));
    let out = send_scheduled(e).await;
    assert_eq!(
        out,
        json!("Scheduled messages: 1 sent, 0 permanently failed")
    );
    assert_eq!(row(e, ok).await.status.as_deref(), Some("sent"));
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].to, vec!["client@example.com".to_string()]);
        assert_eq!(calls[0].subject, "(No Subject)");
        assert_eq!(calls[0].html_body, "Reminder: invoice due tomorrow");
        assert!(calls[0].has_db);
    }
    #[allow(clippy::type_complexity)]
    let stored: Vec<(String, String, Option<String>, Vec<String>, Option<String>, Option<Uuid>)> =
        sqlx::query_as(
            "SELECT direction, from_email, from_name, to_emails, subject, sent_by_id FROM crm_email_messages \
             WHERE thread_id = $1",
        )
        .bind(thread)
        .fetch_all(&mut e.db().await)
        .await
        .unwrap();
    assert_eq!(
        stored,
        vec![(
            "outbound".to_string(),
            "hello@example.test".to_string(),
            Some("Your Company".to_string()),
            vec!["client@example.com".to_string()],
            Some("(No Subject)".to_string()),
            Some(author)
        )]
    );
    // Without a CRM SMTP host nothing is sent and the run counts a failed attempt.
    *host.lock().unwrap() = String::new();
    let second = scheduled(
        e,
        "email",
        author,
        &thread.to_string(),
        "client@example.com",
        Some("Hello"),
        "pending",
        0,
    )
    .await;
    send_scheduled(e).await;
    granary_services::email::set_global_seams(None);
    let r = row(e, second).await;
    assert_eq!(r.status.as_deref(), Some("pending"));
    assert_eq!(r.retry_count, Some(1));
    assert_eq!(r.last_error.as_deref(), Some("CRM SMTP not configured"));
    assert_eq!(calls.lock().unwrap().len(), 1);
    cleanup(e, &[ok, second]).await;
}

#[tokio::test]
async fn missing_scheduling_settings_skip_the_run() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let old: String = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'crm.scheduled.max_attempts'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    sqlx::query("DELETE FROM system_settings WHERE key = 'crm.scheduled.max_attempts'")
        .execute(&mut db)
        .await
        .unwrap();
    let out = send_scheduled(e).await;
    sqlx::query(
        "INSERT INTO system_settings (id, key, value, value_type, category, label, is_public, is_encrypted) \
         VALUES ($1, 'crm.scheduled.max_attempts', $2, 'int', 'crm', 'Scheduled messages: attempts before failing', false, false)",
    )
    .bind(Uuid::new_v4())
    .bind(old)
    .execute(&mut db)
    .await
    .unwrap();
    let text = out.as_str().unwrap();
    assert!(
        text.starts_with("Scheduled messages skipped: Setting 'crm.scheduled.max_attempts' "),
        "{text}"
    );
    assert!(
        text.ends_with("Set it in Settings \u{2192} Sales & CRM \u{2192} Scheduled messages."),
        "{text}"
    );
}

// ─── poll_crm_inbox, nary_dispatch_events ────────────────────────────────────

#[tokio::test]
async fn poll_crm_inbox_without_imap_settings_stores_nothing() {
    let e = the_env();
    let out = e.call("app.tasks.poll_crm_inbox", vec![], 0).await.unwrap();
    assert_eq!(out, Outcome::Done(json!(0)));
}

#[tokio::test]
async fn nary_dispatch_events_returns_the_python_str_of_its_result() {
    let e = the_env();
    let out = e
        .call("app.tasks.nary_dispatch_events", vec![], 0)
        .await
        .unwrap();
    let Outcome::Done(Value::String(s)) = out else {
        panic!("{out:?}")
    };
    assert!(
        s == "{'dispatched': 0, 'paused': True}" || s.starts_with("{'dispatched': "),
        "{s}"
    );
    assert!(
        s.contains("'failed': ") || s.contains("'paused': True"),
        "{s}"
    );
}

// ─── nary_staff_chat_reply ───────────────────────────────────────────────────

/// A linked member of staff whose turn is taken (another answer for them runs).
async fn busy_staff(e: &TestEnv) -> (Uuid, String) {
    let person = user(e, "Busy Staff", false).await;
    let chat = format!("81909{}@c.us", rand_digits());
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, status, staff_user_id) VALUES ($1, $2, $3, 'active', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(&chat)
    .bind(format!("+{}", chat.split('@').next().unwrap()))
    .bind(person)
    .execute(&mut e.db().await)
    .await
    .unwrap();
    let mut r = e.redis().await;
    let _: () = redis::cmd("SET")
        .arg(granary_services::nary::turns::key(&person))
        .arg("someone-else")
        .arg("EX")
        .arg(60)
        .query_async(&mut r)
        .await
        .unwrap();
    (person, chat)
}

#[tokio::test]
async fn a_busy_staff_chat_turn_retries_in_three_seconds() {
    let e = the_env();
    let (_person, chat) = busy_staff(e).await;
    let msg = Uuid::new_v4().to_string();
    for retries in [0, 39] {
        // waited = retries * 3 = 0, 117: below nary.staff_chat.wait_seconds (120), so still busy.
        let out = e
            .call(
                "app.tasks.nary_staff_chat_reply",
                vec![json!(chat), json!(msg)],
                retries,
            )
            .await
            .unwrap();
        assert_eq!(
            out,
            Outcome::Retry {
                countdown: Duration::from_secs(3)
            }
        );
    }
}

#[tokio::test]
async fn the_worker_schedules_the_busy_reply_again_with_its_retry_count() {
    use granary_worker::runtime::{Worker, WorkerOpts};
    let e = the_env();
    let (_person, chat) = busy_staff(e).await;
    let keys = support::keys();
    let mut r = e.redis().await;
    let mut kwargs = Map::new();
    kwargs.insert("late".into(), json!(true));
    support::push(
        &mut r,
        &keys,
        &support::job(
            "app.tasks.nary_staff_chat_reply",
            vec![json!(chat), json!(Uuid::new_v4().to_string())],
            kwargs,
        ),
    )
    .await;
    let mut opts = WorkerOpts::production();
    opts.keys = keys.clone();
    opts.startup_hooks = false;
    opts.concurrency = 1;
    let w = Worker::new(
        e.state.clone(),
        granary_worker::registry::default_registry(),
        opts,
    );
    let (tx, rx) = tokio::sync::watch::channel(false);
    let t0 = Utc::now().timestamp_millis();
    let h = e.spawn(w.run(rx));
    let mut scheduled: Vec<(String, f64)> = vec![];
    for _ in 0..100 {
        scheduled = redis::cmd("ZRANGE")
            .arg(&keys.scheduled)
            .arg(0)
            .arg(-1)
            .arg("WITHSCORES")
            .query_async(&mut r)
            .await
            .unwrap();
        if !scheduled.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = tx.send(true);
    h.await.unwrap().unwrap();
    assert_eq!(
        scheduled.len(),
        1,
        "the busy reply waits in the scheduled set"
    );
    let v: Value = serde_json::from_str(&scheduled[0].0).unwrap();
    assert_eq!(v["task"], "app.tasks.nary_staff_chat_reply");
    assert_eq!(v["kwargs"], json!({"late": true}));
    assert_eq!(v["retries"], 1);
    let due = scheduled[0].1 as i64;
    assert!(
        due >= t0 + 3000 && due <= Utc::now().timestamp_millis() + 3000,
        "due {due}, start {t0}"
    );
}

fn rand_digits() -> String {
    format!("{:08}", Uuid::new_v4().as_u128() % 100_000_000)
}

// ─── _already_ran / _mark_ran ────────────────────────────────────────────────

#[tokio::test]
async fn run_once_markers_are_settings_rows_committed_with_the_session() {
    use granary_worker::tasks::common::{_already_ran, _mark_ran, MARK_RAN_LABEL};
    let e = the_env();
    let key = format!("crm.test_marker.{}", Uuid::new_v4().simple());
    let state = e.state.clone();
    let k = key.clone();
    let (before, after, other_stamp, kept) = e
        .run(async move {
            let db = granary_core::db::DbSession::new(state.db.clone());
            let before = _already_ran(&db, &k, "2026-10-19").await.unwrap();
            // Something else pending in the session is committed with the marker.
            {
                let mut c = db.conn().await.unwrap();
                sqlx::query("INSERT INTO crm_email_threads (id, subject) VALUES ($1, $2)")
                    .bind(Uuid::new_v4())
                    .bind(&k)
                    .execute(&mut *c)
                    .await
                    .unwrap();
            }
            _mark_ran(&db, &k, "2026-10-19", "crm", MARK_RAN_LABEL)
                .await
                .unwrap();
            drop(db);
            let db = granary_core::db::DbSession::new(state.db.clone());
            let after = _already_ran(&db, &k, "2026-10-19").await.unwrap();
            let other = _already_ran(&db, &k, "2026-10-20").await.unwrap();
            _mark_ran(&db, &k, "2026-10-20", "crm", MARK_RAN_LABEL)
                .await
                .unwrap();
            let kept: i64 = {
                let mut c = db.conn().await.unwrap();
                sqlx::query_scalar("SELECT count(*) FROM crm_email_threads WHERE subject = $1")
                    .bind(&k)
                    .fetch_one(&mut *c)
                    .await
                    .unwrap()
            };
            (before, after, other, kept)
        })
        .await;
    assert!(!before);
    assert!(after);
    assert!(!other_stamp);
    assert_eq!(kept, 1);
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT value, value_type, category, label FROM system_settings WHERE key = $1",
    )
    .bind(&key)
    .fetch_all(&mut e.db().await)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![(
            "2026-10-20".to_string(),
            "string".to_string(),
            "crm".to_string(),
            "Internal marker: last follow-up run".to_string()
        )]
    );
}
