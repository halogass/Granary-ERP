//! Port of the service-level tests of backend/tests/test_nary_send_as_nary.py not already in
//! nary_send_rules.rs (the Owner's send past a takeover and quiet hours, Nary's own reply during a takeover) or
//! ai_tools_catalog.rs (the chat card, the full email).
//!
//! Python posts to /api/v1/nary/send (routers/nary.py `send_now`: `_require(user, "nary:approve")`, the body
//! without None/"" values, then `executor.send_on_instruction`). The router is wave 4; here
//! `executor::send_on_instruction` is called as the router calls it. `test_only_the_owner_can_tell_nary_to_send`
//! tests the router's own permission check: wave 4.
//!
//! The `smtp` fixture is the email seam (`_send_via_config` recorded, `load_crm_smtp` a fixed config).

mod nary_n2_common;

use std::sync::{Arc, Mutex};

use granary_core::auth::AuthUser;
use granary_services::email::{self as email_service, Seams, SendCall, SmtpConfig};
use granary_services::nary::executor;
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const TAG: &str = "nary_send_as_nary";
const SUBJECT_TAG: &str = "pytest-send-as-nary";
const TEMPLATE: &str = "<table><tr><td><b>{{name}}</b><br>{{title}}, {{company_name}}<br>{{email}} | {{mobile}}</td></tr></table>";

type Sent = Arc<Mutex<Vec<SendCall>>>;

/// The `smtp` fixture.
fn smtp() -> Sent {
    let sent: Sent = Arc::new(Mutex::new(vec![]));
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
                    from_email: "hello@example.com".into(),
                    tls: true,
                })
            })
        })),
    }));
    sent
}

/// The `template` fixture: the company signature template, switched on (old values back via [`untemplate`]).
async fn template(pool: &PgPool) -> Vec<(String, Option<String>)> {
    let mut old = vec![];
    for (k, v) in [
        ("email.signature.template_html", TEMPLATE),
        ("email.signature.enabled", "true"),
        // the company's contact details on Nary's signature (the baseline ships them empty)
        (
            "nary.email.signature_fields",
            r#"{"name": "Nary", "title": "Sales Assistant", "position": "Sales Assistant", "department": "Sales", "email": "hello@example.com", "mobile": "+1 555 0100", "phone": "+1 555 0100", "city": "Springfield"}"#,
        ),
    ] {
        let prev: Option<Option<String>> =
            sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
                .bind(k)
                .fetch_optional(pool)
                .await
                .unwrap();
        old.push((k.to_string(), prev.flatten()));
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, value_type, category, is_public, label, created_at, updated_at) \
             VALUES (gen_random_uuid(), $1, $2, 'string', 'email', false, $1, now(), now()) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(k)
        .bind(v)
        .execute(pool)
        .await
        .unwrap();
    }
    old
}

async fn untemplate(pool: &PgPool, old: Vec<(String, Option<String>)>) {
    for (k, v) in old {
        match v {
            None => sqlx::query("DELETE FROM system_settings WHERE key = $1")
                .bind(&k)
                .execute(pool)
                .await
                .unwrap(),
            Some(v) => sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
                .bind(&k)
                .bind(v)
                .execute(pool)
                .await
                .unwrap(),
        };
    }
}

/// The `_threads` fixture's cleanup.
async fn drop_threads(pool: &PgPool) {
    let like = format!("%{SUBJECT_TAG}%");
    sqlx::query("DELETE FROM crm_email_messages WHERE thread_id IN (SELECT id FROM crm_email_threads WHERE subject LIKE $1)")
        .bind(&like)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM crm_email_threads WHERE subject LIKE $1")
        .bind(&like)
        .execute(pool)
        .await
        .unwrap();
}

/// `inbound_thread(sender)`: (thread id, from_email, message_id_header).
async fn inbound_thread(pool: &PgPool) -> (Uuid, String, String) {
    let thread = Uuid::new_v4();
    let subject = format!("Follow-Up on the Laptop Order {SUBJECT_TAG}");
    sqlx::query("INSERT INTO crm_email_threads (id, subject, message_count) VALUES ($1, $2, 1)")
        .bind(thread)
        .bind(&subject)
        .execute(pool)
        .await
        .unwrap();
    let from = "adams@globex-pytest.example".to_string();
    let header = format!("<{}@globex.example>", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, from_name, to_emails, \
         subject, body_text, received_at) VALUES ($1, $2, $3, 'inbound', $4, 'Adams', $5, $6, \
         'We need attendance for 300 staff.', now())",
    )
    .bind(Uuid::new_v4())
    .bind(thread)
    .bind(&header)
    .bind(&from)
    .bind(vec!["hello@example.com".to_string()])
    .bind(&subject)
    .execute(pool)
    .await
    .unwrap();
    (thread, from, header)
}

async fn me(pool: &PgPool) -> AuthUser {
    let id = owner(pool).await;
    let mut c = pool.acquire().await.unwrap();
    auth_user(&mut c, id).await
}

/// `send_now(token, body)` without the router: `executor.send_on_instruction(db, user, f"{channel}.reply.send",
/// body)`.
async fn send_now(
    pool: &PgPool,
    user: &AuthUser,
    body: Value,
) -> granary_core::errors::ApiResult<granary_db::rows::nary::AgentPendingAction> {
    let channel = body["channel"].as_str().unwrap().to_string();
    let mut c = pool.acquire().await.unwrap();
    executor::send_on_instruction(&mut c, user, &format!("{channel}.reply.send"), body, None).await
}

fn header(call: &SendCall, name: &str) -> Option<String> {
    call.headers
        .as_ref()?
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

fn run_email<F, Fut>(f: F)
where
    F: FnOnce(PgPool, Sent) -> Fut + Send,
    Fut: std::future::Future<Output = ()>,
{
    run(TAG, |pool| async move {
        working_hours(true);
        let sent = smtp();
        f(pool.clone(), sent).await;
        email_service::set_global_seams(None);
        drop_threads(&pool).await;
    });
}

// ─── Email ────────────────────────────────────────────────────────────────────

#[test]
fn test_an_email_is_a_reply_in_their_thread_with_the_company_signature_as_nary() {
    run_email(|pool, smtp| async move {
        let old = template(&pool).await;
        let user = me(&pool).await;
        let (thread, from, msg_header) = inbound_thread(&pool).await;
        let row = send_now(
            &pool,
            &user,
            json!({
            "channel": "email", "thread_id": thread.to_string(), "to": from,
            "text": "Dear Mr. Smith,\n\nThank you for following up.\n\nBest regards,\n[Name]"}),
        )
        .await;
        untemplate(&pool, old).await;
        let row = row.unwrap();
        assert_eq!(row.status, "applied");
        assert_eq!(row.origin, "chat");
        {
            let calls = smtp.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let mail = &calls[0];
            assert_eq!(mail.to, vec![from.clone()]);
            assert!(
                mail.subject
                    .starts_with("Re: Follow-Up on the Laptop Order"),
                "{}",
                mail.subject
            );
            assert_eq!(
                header(mail, "In-Reply-To").as_deref(),
                Some(msg_header.as_str())
            );
            let html = &mail.html_body;
            assert!(
                html.contains("Thank you for following up.") && html.contains("Best regards,"),
                "{html}"
            );
            assert!(!html.contains("[Name]"), "{html}"); // the model's placeholder is dropped
            assert!(
                html.contains("<b>Nary</b>")
                    && html.contains("Sales Assistant")
                    && html.contains("data-granary-signature"),
                "{html}"
            );
        }
        let in_reply_to: Option<String> = sqlx::query_scalar(
            "SELECT in_reply_to FROM crm_email_messages WHERE thread_id = $1 AND direction = 'outbound'",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(in_reply_to.as_deref(), Some(msg_header.as_str())); // stored in the same conversation
    });
}

#[test]
fn test_a_missing_signature_field_is_a_clear_error() {
    run_email(|pool, smtp| async move {
        let old = template(&pool).await;
        let user = me(&pool).await;
        let (thread, from, _) = inbound_thread(&pool).await;
        let old_fields = set(
            &pool,
            &[("nary.email.signature_fields", r#"{"name": "Nary"}"#)],
        )
        .await;
        let r = send_now(
            &pool,
            &user,
            json!({"channel": "email", "thread_id": thread.to_string(), "to": from,
                                              "text": "Hello"}),
        )
        .await;
        restore(&pool, old_fields).await;
        untemplate(&pool, old).await;
        let e = r.unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(e.status().as_u16() >= 400, "{text}");
        assert!(
            text.contains("nary.email.signature_fields") && text.contains("title"),
            "{text}"
        );
        assert!(smtp.lock().unwrap().is_empty());
    });
}

#[test]
fn test_without_a_template_the_email_ends_with_a_plain_sign_off() {
    run_email(|pool, smtp| async move {
        // no company template (a fresh database has none; Python's test runs without the `template` fixture)
        sqlx::query("DELETE FROM system_settings WHERE key IN ('email.signature.template_html', 'email.signature.enabled')")
            .execute(&pool)
            .await
            .unwrap();
        let user = me(&pool).await;
        let row = send_now(
            &pool,
            &user,
            json!({"channel": "email", "to": "nick@globex-pytest.example",
                                                "subject": format!("Laptop packages {SUBJECT_TAG}"),
                                                "text": "Hi Nathan,\n\nHere is the overview."}),
        )
        .await
        .unwrap();
        assert_eq!(row.status, "applied");
        let calls = smtp.lock().unwrap();
        assert!(
            calls[0].html_body.contains("Nary, Your Company"),
            "{}",
            calls[0].html_body
        );
        assert!(header(&calls[0], "In-Reply-To").is_none());
    });
}

// ─── WhatsApp ─────────────────────────────────────────────────────────────────

#[test]
#[ignore = "covered by crates/api/tests/nary_routes.rs::test_only_the_owner_can_tell_nary_to_send (wave 4)"]
fn test_only_the_owner_can_tell_nary_to_send() {}

#[test]
fn test_in_shadow_mode_narys_replies_stay_drafts_but_the_ceos_send_goes() {
    run(TAG, |pool| async move {
        working_hours(true);
        let user = me(&pool).await;
        let cid = chat_id();
        ingest(
            &pool,
            &payload(&cid, DEFAULT_BODY, false, None, None, json!({})),
            "live",
        )
        .await;
        let old = set(&pool, &[("nary.shadow_mode", "true")]).await;
        let draft = {
            let mut c = pool.acquire().await.unwrap();
            executor::propose(
                &mut c,
                "whatsapp.reply.send",
                json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello Mr"}),
                &user,
                executor::ProposeOpts::default(),
            )
            .await
            .unwrap()
        };
        assert_eq!(draft.status, "shadow");
        let r = send_now(
            &pool,
            &user,
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello Mr"}),
        )
        .await;
        restore(&pool, old).await;
        r.unwrap();
        assert_eq!(sent().len(), 1);
    });
}
