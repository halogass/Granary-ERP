//! backend/tests/test_email_marketing.py through the real app: campaigns are created and dispatched over HTTP
//! (routers/marketing.py), the queue worker (`_process_next`) runs with a fake SMTP backend exactly as the
//! Python `smtp` fixture monkeypatches it, and the unsubscribe / resubscribe links are called without auth.
//! The HTTP-only tests of that file (preview, senders, access matrix) are in the HTTP suite
//! (compat-tests/http-suite/tests/test_marketing_email.py); `test_the_button_colour_must_be_a_css_colour` is a
//! pure renderer test covered by crates/services/tests/email_marketing_golden.rs.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::email_marketing::{self as em, MailBackend, SmtpCfg};
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp, TestUser, UserSpec};
use uuid::Uuid;

const TAG: &str = "marketing_email";
const E: &str = "/api/v1/marketing/email";
const SIGNATURE_TEMPLATE: &str =
    "<p class=\"sig\">Best regards,<br>{{name}} &middot; {{email}}</p>";

#[derive(Debug, Clone)]
struct Sent {
    to: String,
    subject: String,
    html: String,
    headers: Vec<(String, String)>,
}

#[derive(Default, Clone)]
struct FakeSmtp {
    sent: Arc<Mutex<Vec<Sent>>>,
    /// `_send_via_config` returns False (SMTP not configured).
    not_configured: bool,
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// MailBackend is an #[async_trait] trait; written out by hand (this crate has no async-trait dependency).
impl MailBackend for FakeSmtp {
    fn load_crm_smtp<'a, 'b, 'c>(
        &'a self,
        _db: &'b mut PgConnection,
    ) -> BoxFut<'c, anyhow::Result<SmtpCfg>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            Ok(SmtpCfg {
                host: "smtp.test".into(),
                port: 587,
                user: String::new(),
                password: String::new(),
                from_name: "ACME".into(),
                from_email: "news@granary.test".into(),
                tls: true,
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn send_via_config<'a, 'b, 'c, 'd, 'e, 'f, 'g, 'h>(
        &'a self,
        _cfg: &'b SmtpCfg,
        to: &'c str,
        subject: &'d str,
        html_body: &'e str,
        _db: &'f mut PgConnection,
        headers: &'g [(String, String)],
    ) -> BoxFut<'h, anyhow::Result<bool>>
    where
        'a: 'h,
        'b: 'h,
        'c: 'h,
        'd: 'h,
        'e: 'h,
        'f: 'h,
        'g: 'h,
        Self: 'h,
    {
        Box::pin(async move {
            if self.not_configured {
                return Ok(false);
            }
            self.sent.lock().unwrap().push(Sent {
                to: to.into(),
                subject: subject.into(),
                html: html_body.into(),
                headers: headers.to_vec(),
            });
            Ok(true)
        })
    }

    fn sent_today<'a, 'b, 'c>(
        &'a self,
        _db: &'b mut PgConnection,
    ) -> BoxFut<'c, anyhow::Result<i64>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async move { Ok(0) })
    }
}

impl FakeSmtp {
    fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

fn blocks() -> Value {
    json!([{"type": "heading", "level": 1, "text": "Hello {{first_name}}"},
           {"type": "paragraph", "text": "News for {{company}} ({{email}})."}])
}

async fn marketer(app: &TestApp) -> TestUser {
    app.make_user(UserSpec::new().perms(&["marketing:email"]))
        .await
}

/// `_list_with(*contacts)`: a list and its subscribed contacts, written directly as the Python helper does.
async fn list_with(app: &TestApp, contacts: &[(&str, &str, &str)]) -> String {
    let mut db = app.db().await;
    let ml = Uuid::new_v4();
    sqlx::query("INSERT INTO mailing_lists (id, name, is_deleted) VALUES ($1, $2, false)")
        .bind(ml)
        .bind(u("Pytest list"))
        .execute(&mut db)
        .await
        .unwrap();
    for (email, first, company) in contacts {
        sqlx::query(
            "INSERT INTO mailing_list_contacts (id, list_id, email, first_name, last_name, company, is_subscribed) \
             VALUES ($1, $2, $3, $4, 'Tester', $5, true)",
        )
        .bind(Uuid::new_v4())
        .bind(ml)
        .bind(email)
        .bind(first)
        .bind(company)
        .execute(&mut db)
        .await
        .unwrap();
    }
    ml.to_string()
}

/// `_campaign(token, list_ids, **extra)`.
async fn campaign(app: &TestApp, token: &str, lists: &[&str], extra: Value) -> String {
    let mut body = json!({"name": u("Pytest campaign"), "subject": "Hi {{first_name}}", "blocks": blocks(),
                          "list_ids": lists});
    if let Value::Object(m) = extra {
        for (k, v) in m {
            body[k] = v;
        }
    }
    let r = app.post(token, &format!("{E}/campaigns"), body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["id"].as_str().unwrap().to_string()
}

async fn dispatch(app: &TestApp, token: &str, cid: &str) {
    let r = app
        .post(
            token,
            &format!("{E}/campaigns/{cid}/dispatch"),
            json!({"send_now": true}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

/// One `_process_next` pass with every other sending/scheduled campaign paused.
async fn process_once(app: &TestApp, cid: &str, smtp: &FakeSmtp) {
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE email_campaigns SET status = 'paused_by_test' WHERE status IN ('sending', 'scheduled') \
         AND id <> $1::uuid",
    )
    .bind(cid)
    .execute(&mut db)
    .await
    .unwrap();
    em::_process_next_with(&mut db, smtp).await.unwrap();
}

/// `_dispatch_and_drain(token, campaign_id, dispatch=...)`.
async fn dispatch_and_drain(
    app: &TestApp,
    token: &str,
    cid: &str,
    smtp: &FakeSmtp,
    do_dispatch: bool,
) {
    if do_dispatch {
        dispatch(app, token, cid).await;
    }
    for _ in 0..20 {
        let mut db = app.db().await;
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM email_queue_items WHERE campaign_id = $1::uuid AND status = 'pending'",
        )
        .bind(cid)
        .fetch_one(&mut db)
        .await
        .unwrap();
        if pending == 0 {
            return;
        }
        process_once(app, cid, smtp).await;
    }
}

fn header<'a>(m: &'a Sent, k: &str) -> &'a str {
    m.headers
        .iter()
        .find(|(h, _)| h == k)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

async fn with_signature_template(app: &TestApp) {
    app.set_setting(
        granary_services::email_signature::TEMPLATE_KEY,
        SIGNATURE_TEMPLATE,
    )
    .await;
    em::signature_cache_clear();
}

// ─── Footer: signature + unsubscribe ─────────────────────────────────────────

#[tokio::test]
async fn test_campaign_email_closes_with_the_sender_signature_and_one_click_unsubscribe() {
    let app = app(TAG);
    let _g = app.serial().await;
    with_signature_template(app).await;
    let smtp = FakeSmtp::default();
    let sender = marketer(app).await;
    let ann = format!("{}@customer.test", u("ann"));
    let list_id = list_with(app, &[(&ann, "Ann", "Acme")]).await;
    let cid = campaign(
        app,
        &sender.token,
        &[&list_id],
        json!({"sender_user_id": sender.id.to_string()}),
    )
    .await;
    dispatch_and_drain(app, &sender.token, &cid, &smtp, true).await;

    let sent = smtp.sent();
    assert_eq!(sent.len(), 1);
    let mail = &sent[0];
    assert_eq!(mail.subject, "Hi Ann");
    assert!(mail.html.contains("Hello Ann") && mail.html.contains("News for Acme"));
    assert!(
        mail.html.contains("class=\"sig\"") && mail.html.contains("Best regards"),
        "the sender's signature closes the email"
    );
    assert!(mail.html.contains("/unsubscribe?token=") && mail.html.contains("all marketing email"));
    assert_eq!(
        header(mail, "List-Unsubscribe-Post"),
        "List-Unsubscribe=One-Click"
    );
    assert!(header(mail, "List-Unsubscribe").contains("/api/v1/marketing/email/unsubscribe/"));
}

#[tokio::test]
async fn test_without_a_signature_the_company_line_closes_the_email() {
    let app = app(TAG);
    let _g = app.serial().await;
    em::signature_cache_clear();
    let smtp = FakeSmtp::default();
    let sender = marketer(app).await;
    let bob = format!("{}@customer.test", u("bob"));
    let list_id = list_with(app, &[(&bob, "Bob", "Beta")]).await;
    let cid = campaign(
        app,
        &sender.token,
        &[&list_id],
        json!({"sender_user_id": sender.id.to_string(), "include_signature": false}),
    )
    .await;
    dispatch_and_drain(app, &sender.token, &cid, &smtp, true).await;
    let sent = smtp.sent();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].html.contains("class=\"sig\"") && sent[0].html.contains("Unsubscribe"));
}

// ─── Global unsubscribe ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_one_click_unsubscribe_opts_out_of_every_list() {
    let app = app(TAG);
    let _g = app.serial().await;
    em::signature_cache_clear();
    let smtp = FakeSmtp::default();
    let sender = marketer(app).await;
    let email = format!("{}@customer.test", u("cat"));
    let dan = format!("{}@customer.test", u("dan"));
    let list_a = list_with(app, &[(&email, "Cat", "Gamma")]).await;
    let list_b = list_with(app, &[(&email, "Cat", "Gamma"), (&dan, "Dan", "Delta")]).await;
    let first = campaign(app, &sender.token, &[&list_a], json!({})).await;
    dispatch_and_drain(app, &sender.token, &first, &smtp, true).await;
    let mut db = app.db().await;
    let token: String = sqlx::query_scalar(
        "SELECT tracking_id FROM email_queue_items WHERE campaign_id = $1::uuid",
    )
    .bind(&first)
    .fetch_one(&mut db)
    .await
    .unwrap();

    let r = app
        .post("", &format!("{E}/unsubscribe/{token}"), json!(null))
        .await; // what Gmail's button does
    let again = app
        .post("", &format!("{E}/unsubscribe/{token}"), json!(null))
        .await; // idempotent
    assert_eq!(r.json()["success"], true);
    assert_eq!(again.json()["success"], true);

    let smtp2 = FakeSmtp::default();
    let second = campaign(app, &sender.token, &[&list_b], json!({})).await;
    dispatch_and_drain(app, &sender.token, &second, &smtp2, true).await;
    let sent = smtp2.sent();
    assert!(!sent.iter().any(|m| m.to == email) && sent.len() == 1);

    // Re-adding the address to a list keeps it unsubscribed.
    let added = app
        .post(
            &sender.token,
            &format!("{E}/lists/{list_b}/contacts"),
            json!({"email": email.to_uppercase()}),
        )
        .await
        .json();
    assert_eq!(added["is_subscribed"], false);
    assert_eq!(added["suppressed"], true);

    let res = app
        .post("", &format!("{E}/resubscribe/{token}"), json!(null))
        .await;
    assert_eq!(res.status, 200);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM email_suppressions WHERE email = $1")
        .bind(&email)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let subs: Vec<bool> =
        sqlx::query_scalar("SELECT is_subscribed FROM mailing_list_contacts WHERE email = $1")
            .bind(&email)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert!(!subs.is_empty() && subs.iter().all(|s| *s));
}

#[tokio::test]
async fn test_an_address_that_unsubscribes_after_queueing_is_skipped() {
    let app = app(TAG);
    let _g = app.serial().await;
    let smtp = FakeSmtp::default();
    let sender = marketer(app).await;
    let email = format!("{}@customer.test", u("eve"));
    let list = list_with(app, &[(&email, "Eve", "Epsilon")]).await;
    let cid = campaign(app, &sender.token, &[&list], json!({})).await;
    dispatch(app, &sender.token, &cid).await;
    let mut db = app.db().await;
    sqlx::query("INSERT INTO email_suppressions (id, email, reason) VALUES ($1, $2, 'manual')")
        .bind(Uuid::new_v4())
        .bind(&email)
        .execute(&mut db)
        .await
        .unwrap();
    dispatch_and_drain(app, &sender.token, &cid, &smtp, false).await;
    assert!(smtp.sent().is_empty());
    let status: String =
        sqlx::query_scalar("SELECT status FROM email_queue_items WHERE campaign_id = $1::uuid")
            .bind(&cid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(status, "skipped");
}

async fn queue_state(app: &TestApp, cid: &str) -> (String, bool, i32, String, Option<String>, i32) {
    let mut db = app.db().await;
    let (status, sent_at_none, error): (String, bool, Option<String>) = sqlx::query_as(
        "SELECT status, sent_at IS NULL, error_message FROM email_queue_items WHERE campaign_id = $1::uuid",
    )
    .bind(cid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let (sent, cstatus, failed): (Option<i32>, Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT sent_count, status, failed_count FROM email_campaigns WHERE id = $1::uuid",
    )
    .bind(cid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    (
        status,
        sent_at_none,
        sent.unwrap_or(0),
        cstatus.unwrap_or_default(),
        error,
        failed.unwrap_or(0),
    )
}

#[tokio::test]
async fn test_an_email_is_not_counted_as_sent_when_smtp_is_not_configured() {
    let app = app(TAG);
    let _g = app.serial().await;
    let smtp = FakeSmtp {
        not_configured: true,
        ..Default::default()
    };
    let sender = marketer(app).await;
    let gil = format!("{}@customer.test", u("gil"));
    let list = list_with(app, &[(&gil, "Gil", "Eta")]).await;
    let cid = campaign(app, &sender.token, &[&list], json!({})).await;
    dispatch(app, &sender.token, &cid).await;
    process_once(app, &cid, &smtp).await;
    let (status, sent_at_none, sent, cstatus, _, _) = queue_state(app, &cid).await;
    assert!(
        status == "pending" && sent_at_none,
        "nothing went out: it waits for SMTP"
    );
    assert_eq!((sent, cstatus.as_str()), (0, "sending"));
}

#[tokio::test]
async fn test_a_block_that_cannot_render_fails_its_email_instead_of_retrying_forever() {
    let app = app(TAG);
    let _g = app.serial().await;
    let smtp = FakeSmtp::default();
    let sender = marketer(app).await;
    let hal = format!("{}@customer.test", u("hal"));
    let list = list_with(app, &[(&hal, "Hal", "Theta")]).await;
    let cid = campaign(
        app,
        &sender.token,
        &[&list],
        json!({"blocks": [{"type": "spacer", "height": "abc"}]}),
    )
    .await;
    dispatch(app, &sender.token, &cid).await;
    process_once(app, &cid, &smtp).await;
    let (status, _, _, cstatus, error, failed) = queue_state(app, &cid).await;
    let error = error.unwrap_or_default();
    assert!(status == "failed" && error.contains("could not be rendered") && error.contains("abc"));
    assert_eq!(
        (failed, cstatus.as_str()),
        (1, "sent"),
        "nothing left pending: the campaign closes"
    );
    assert!(smtp.sent().is_empty());
}

// ─── Preview ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_preview_renders_the_whole_email_for_a_list_contact_and_sends_nothing() {
    let app = app(TAG);
    let _g = app.serial().await;
    with_signature_template(app).await;
    let sender = marketer(app).await;
    let fay = format!("{}@customer.test", u("fay"));
    let list_id = list_with(app, &[(&fay, "Fay", "Zeta Corp")]).await;
    let r = app
        .post(
            &sender.token,
            &format!("{E}/preview"),
            json!({"subject": "Hi {{first_name}}", "blocks": blocks(), "list_ids": [list_id],
                   "sender_user_id": sender.id.to_string(), "include_signature": true}),
        )
        .await;
    let sample = app
        .post(
            &sender.token,
            &format!("{E}/preview"),
            json!({"subject": "Hi {{first_name}}", "blocks": blocks(), "list_ids": [], "include_signature": false}),
        )
        .await
        .json();
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let html = body["html"].as_str().unwrap();
    assert_eq!(body["subject"], "Hi Fay");
    assert!(html.contains("News for Zeta Corp"));
    assert_eq!(body["signature_included"], true);
    assert!(html.contains("class=\"sig\""));
    assert!(!html.contains("/track/") && !html.contains("/unsubscribe?token="));
    assert_eq!(sample["recipient"]["sample"], true);
    assert_eq!(sample["subject"], "Hi Jane");
    let mut db = app.db().await;
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM email_queue_items q JOIN mailing_list_contacts c ON q.contact_id = c.id \
         WHERE c.list_id = $1::uuid",
    )
    .bind(&list_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(queued, 0);
}

#[tokio::test]
async fn test_senders_list_flags_who_has_a_signature() {
    let app = app(TAG);
    let _g = app.serial().await;
    with_signature_template(app).await;
    let me = marketer(app).await;
    let senders = app.get(&me.token, &format!("{E}/senders")).await.json();
    let mine = senders
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == me.id.to_string())
        .unwrap()
        .clone();
    assert_eq!(mine["is_me"], true);
    assert_eq!(mine["has_signature"], true);
}

/// (perms, role, department, superadmin, allowed)
type AccessCase = (
    &'static [&'static str],
    Option<&'static str>,
    Option<&'static str>,
    bool,
    bool,
);

#[tokio::test]
async fn test_email_marketing_is_limited_to_the_marketing_team_and_the_owner() {
    let app = app(TAG);
    // (perms, role, department, superadmin, allowed)
    let cases: [AccessCase; 6] = [
        (&["marketing:email"], None, None, false, true),
        (&[], Some("team_lead"), Some("Marketing"), false, true),
        (&[], None, None, true, true),
        (&[], Some("team_lead"), Some("Engineering"), false, false),
        (
            &["marketing:view", "marketing:manage_campaigns"],
            None,
            None,
            false,
            false,
        ),
        (&[], None, None, false, false),
    ];
    for (perms, role, department, superadmin, allowed) in cases {
        let mut spec = UserSpec::new().perms(perms);
        if let Some(r) = role {
            spec = spec.roles(&[r]);
        }
        if superadmin {
            spec = spec.superadmin();
        }
        let person = app.make_user(spec).await;
        let mut db = app.db().await;
        sqlx::query(
            "INSERT INTO employees (id, first_name, email, status, user_id, department, is_deleted) \
             VALUES ($1, 'Pytest', $2, 'active', $3, $4, false)",
        )
        .bind(Uuid::new_v4())
        .bind(person.email.clone())
        .bind(person.id)
        .bind(department)
        .execute(&mut db)
        .await
        .unwrap();
        let access = app.get(&person.token, &format!("{E}/access")).await.json();
        let lists = app.get(&person.token, &format!("{E}/lists")).await.status;
        let created = app
            .post(
                &person.token,
                &format!("{E}/lists"),
                json!({"name": u("Pytest access list")}),
            )
            .await
            .status;
        let campaigns = app
            .get(&person.token, &format!("{E}/campaigns"))
            .await
            .status;
        assert_eq!(access, json!({"allowed": allowed}));
        let expected = if allowed {
            (200, 201, 200)
        } else {
            (403, 403, 403)
        };
        assert_eq!((lists, created, campaigns), expected);
    }
}
