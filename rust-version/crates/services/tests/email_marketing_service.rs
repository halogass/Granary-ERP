//! Ports of backend/tests/test_email_marketing.py and the queue-worker tests of test_audit_medium_w3.py
//! (M53), at the service level: the Python tests create and dispatch campaigns over HTTP, whose handlers are
//! thin wrappers around `dispatch_campaign`; here the campaign row is inserted as the create endpoint writes
//! it and `dispatch_campaign` is called directly. The SMTP fakes replace `load_crm_smtp`, `_send_via_config`
//! and `_sent_today` exactly as the Python `smtp` fixture monkeypatches them.
//!
//! Not ported here (router-only, no service code involved): the unsubscribe/resubscribe endpoints'
//! own effects, the senders list and the access matrix (test_email_marketing_is_limited_to_...).

mod support;

use std::sync::{Arc, Mutex};

use granary_db::rows::marketing::EmailCampaign;
use granary_services::email_marketing::{self as em, MailBackend, SignatureCampaign, SmtpCfg};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

const SIGNATURE_TEMPLATE: &str =
    "<p class=\"sig\">Best regards,<br>{{name}} &middot; {{email}}</p>";

fn init() {
    std::env::set_var("REDIS_URL", "redis://127.0.0.1:56380/11");
    support::init();
}

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

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
    /// `_send_via_config` returns False (SMTP not configured) and sends nothing.
    not_configured: bool,
}

#[async_trait::async_trait]
impl MailBackend for FakeSmtp {
    async fn load_crm_smtp(&self, _db: &mut PgConnection) -> anyhow::Result<SmtpCfg> {
        Ok(SmtpCfg {
            host: "smtp.test".into(),
            port: 587,
            user: String::new(),
            password: String::new(),
            from_name: "ACME".into(),
            from_email: "news@example.com".into(),
            tls: true,
        })
    }

    async fn send_via_config(
        &self,
        _cfg: &SmtpCfg,
        to: &str,
        subject: &str,
        html_body: &str,
        _db: &mut PgConnection,
        headers: &[(String, String)],
    ) -> anyhow::Result<bool> {
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
    }

    async fn sent_today(&self, _db: &mut PgConnection) -> anyhow::Result<i64> {
        Ok(0) // no_limit
    }
}

impl FakeSmtp {
    fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

async fn setup(tag: &str) -> PgPool {
    init();
    em::signature_cache_clear();
    support::fresh_db(&format!("s6_em_{tag}")).await
}

async fn signature_template(pool: &PgPool) {
    let n = sqlx::query("UPDATE system_settings SET value = $1 WHERE key = $2")
        .bind(SIGNATURE_TEMPLATE)
        .bind(granary_services::email_signature::TEMPLATE_KEY)
        .execute(pool)
        .await
        .unwrap()
        .rows_affected();
    if n == 0 {
        sqlx::query("INSERT INTO system_settings (id, key, value, category, label) VALUES ($1, $2, $3, 'email', 'Signature')")
            .bind(Uuid::new_v4())
            .bind(granary_services::email_signature::TEMPLATE_KEY)
            .bind(SIGNATURE_TEMPLATE)
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn make_person(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@example.com", u("pytest.approver")))
    .bind(u("Pytest Person"))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn list_with(pool: &PgPool, contacts: &[(&str, &str, &str)]) -> Uuid {
    let ml = Uuid::new_v4();
    sqlx::query("INSERT INTO mailing_lists (id, name, is_deleted) VALUES ($1, $2, false)")
        .bind(ml)
        .bind(u("Pytest list"))
        .execute(pool)
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
        .execute(pool)
        .await
        .unwrap();
    }
    ml
}

fn blocks() -> Value {
    json!([{"type": "heading", "level": 1, "text": "Hello {{first_name}}"},
           {"type": "paragraph", "text": "News for {{company}} ({{email}})."}])
}

/// What POST /marketing/email/campaigns writes.
async fn campaign(
    pool: &PgPool,
    creator: Uuid,
    lists: &[Uuid],
    sender: Option<Uuid>,
    include_signature: bool,
) -> Uuid {
    campaign_with(pool, creator, lists, sender, include_signature, blocks()).await
}

async fn campaign_with(
    pool: &PgPool,
    creator: Uuid,
    lists: &[Uuid],
    sender: Option<Uuid>,
    include_signature: bool,
    blocks: Value,
) -> Uuid {
    let id = Uuid::new_v4();
    let list_ids: Vec<String> = lists.iter().map(|l| l.to_string()).collect();
    sqlx::query(
        "INSERT INTO email_campaigns (id, name, subject, blocks, list_ids, status, created_by_id, sender_user_id, \
         include_signature, is_deleted, total_recipients, sent_count, failed_count, open_count) \
         VALUES ($1, $2, 'Hi {{first_name}}', $3, $4, 'draft', $5, $6, $7, false, 0, 0, 0, 0)",
    )
    .bind(id)
    .bind(u("Pytest campaign"))
    .bind(blocks)
    .bind(json!(list_ids))
    .bind(creator)
    .bind(sender)
    .bind(include_signature)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn load_campaign(pool: &PgPool, id: Uuid) -> EmailCampaign {
    sqlx::query_as("SELECT * FROM email_campaigns WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn dispatch(pool: &PgPool, id: Uuid) -> usize {
    let mut c = load_campaign(pool, id).await;
    let mut conn = pool.acquire().await.unwrap();
    em::dispatch_campaign(&mut c, &mut conn, None)
        .await
        .unwrap()
}

/// `_dispatch_and_drain(..., dispatch=False)`: process until this campaign has no pending item.
async fn drain(pool: &PgPool, id: Uuid, backend: &FakeSmtp) {
    for _ in 0..20 {
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM email_queue_items WHERE campaign_id = $1 AND status = 'pending'",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
        if pending == 0 {
            return;
        }
        sqlx::query("UPDATE email_campaigns SET status = 'paused_by_test' WHERE status IN ('sending', 'scheduled') AND id <> $1")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        em::_process_next_with(&mut conn, backend).await.unwrap();
    }
}

fn header<'a>(m: &'a Sent, k: &str) -> &'a str {
    m.headers
        .iter()
        .find(|(h, _)| h == k)
        .map(|(_, v)| v.as_str())
        .unwrap()
}

// ─── Footer: signature + unsubscribe ──────────────────────────────────────────

#[tokio::test]
async fn test_campaign_email_closes_with_the_sender_signature_and_one_click_unsubscribe() {
    let pool = setup("footer_sig").await;
    signature_template(&pool).await;
    let smtp = FakeSmtp::default();
    let sender = make_person(&pool).await;
    let ann = format!("{}@customer.test", u("ann"));
    let list_id = list_with(&pool, &[(&ann, "Ann", "Acme")]).await;
    let cid = campaign(&pool, sender, &[list_id], Some(sender), true).await;
    assert_eq!(dispatch(&pool, cid).await, 1);
    drain(&pool, cid, &smtp).await;

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

    // The queue item, the counters and the closed campaign.
    let (status, tracking): (String, String) =
        sqlx::query_as("SELECT status, tracking_id FROM email_queue_items WHERE campaign_id = $1")
            .bind(cid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "sent");
    assert_eq!(tracking.len(), 36);
    assert!(mail
        .html
        .contains(&format!("/unsubscribe?token={tracking}")));
    assert_eq!(
        header(mail, "List-Unsubscribe"),
        format!(
            "<{}/api/v1/marketing/email/unsubscribe/{tracking}>",
            granary_core::config::settings().API_URL
        )
    );
    let c = load_campaign(&pool, cid).await;
    assert_eq!(
        (
            c.status.as_deref(),
            c.sent_count,
            c.failed_count,
            c.total_recipients
        ),
        (Some("sent"), Some(1), Some(0), Some(1))
    );
    assert!(c.sent_at.is_some());
}

#[tokio::test]
async fn test_without_a_signature_the_company_line_closes_the_email() {
    let pool = setup("footer_company").await;
    let smtp = FakeSmtp::default();
    let sender = make_person(&pool).await;
    let bob = format!("{}@customer.test", u("bob"));
    let list_id = list_with(&pool, &[(&bob, "Bob", "Beta")]).await;
    let cid = campaign(&pool, sender, &[list_id], Some(sender), false).await;
    dispatch(&pool, cid).await;
    drain(&pool, cid, &smtp).await;
    let sent = smtp.sent();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].html.contains("class=\"sig\"") && sent[0].html.contains("Unsubscribe"));
    assert!(
        sent[0]
            .html
            .contains("<strong>Your Company</strong> &middot; hello@example.com"),
        "{}",
        sent[0].html
    );
}

// ─── Global unsubscribe ───────────────────────────────────────────────────────

/// The one-click endpoint writes an `email_suppressions` row (lower-cased address); every later dispatch
/// leaves that address out, whatever list it is on and however it is cased there.
#[tokio::test]
async fn test_one_click_unsubscribe_opts_out_of_every_list() {
    let pool = setup("unsub_all").await;
    let smtp = FakeSmtp::default();
    let sender = make_person(&pool).await;
    let cat = format!("{}@customer.test", u("cat"));
    let dan = format!("{}@customer.test", u("dan"));
    let list_a = list_with(&pool, &[(&cat, "Cat", "Gamma")]).await;
    let list_b = list_with(
        &pool,
        &[
            (&cat.to_uppercase(), "Cat", "Gamma"),
            (&dan, "Dan", "Delta"),
        ],
    )
    .await;
    let first = campaign(&pool, sender, &[list_a], None, true).await;
    dispatch(&pool, first).await;
    drain(&pool, first, &smtp).await;
    assert_eq!(smtp.sent().len(), 1);

    sqlx::query("INSERT INTO email_suppressions (id, email, reason, campaign_id) VALUES ($1, $2, 'unsubscribe', $3)")
        .bind(Uuid::new_v4())
        .bind(&cat)
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();

    smtp.sent.lock().unwrap().clear();
    let second = campaign(&pool, sender, &[list_b], None, true).await;
    assert_eq!(dispatch(&pool, second).await, 1);
    drain(&pool, second, &smtp).await;
    let to: Vec<String> = smtp.sent().into_iter().map(|m| m.to).collect();
    assert_eq!(to, vec![dan]);
}

#[tokio::test]
async fn test_an_address_that_unsubscribes_after_queueing_is_skipped() {
    let pool = setup("unsub_late").await;
    let smtp = FakeSmtp::default();
    let sender = make_person(&pool).await;
    let eve = format!("{}@customer.test", u("eve"));
    let cid = campaign(
        &pool,
        sender,
        &[list_with(&pool, &[(&eve, "Eve", "Epsilon")]).await],
        None,
        true,
    )
    .await;
    dispatch(&pool, cid).await;
    sqlx::query("INSERT INTO email_suppressions (id, email, reason) VALUES ($1, $2, 'manual')")
        .bind(Uuid::new_v4())
        .bind(&eve)
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool, cid, &smtp).await;
    assert!(smtp.sent().is_empty());
    let (status, err): (String, Option<String>) = sqlx::query_as(
        "SELECT status, error_message FROM email_queue_items WHERE campaign_id = $1",
    )
    .bind(cid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "skipped");
    assert_eq!(err.as_deref(), Some("Unsubscribed from marketing email"));
    assert_eq!(
        load_campaign(&pool, cid).await.status.as_deref(),
        Some("sent")
    );
}

/// `_process_once`: dispatch the campaign and run one pass of the queue worker, with every other campaign
/// paused.
async fn process_once(pool: &PgPool, id: Uuid, backend: &FakeSmtp) {
    dispatch(pool, id).await;
    sqlx::query("UPDATE email_campaigns SET status = 'paused_by_test' WHERE status IN ('sending', 'scheduled') AND id <> $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    em::_process_next_with(&mut conn, backend).await.unwrap();
}

/// `_queue_state`: the campaign's single queue item (status, sent_at, error) and the campaign.
async fn queue_state(
    pool: &PgPool,
    id: Uuid,
) -> (
    (
        String,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<String>,
    ),
    EmailCampaign,
) {
    let item = sqlx::query_as(
        "SELECT status, sent_at, error_message FROM email_queue_items WHERE campaign_id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    (item, load_campaign(pool, id).await)
}

#[tokio::test]
async fn test_an_email_is_not_counted_as_sent_when_smtp_is_not_configured() {
    let pool = setup("smtp_unconfigured").await;
    let smtp = FakeSmtp {
        not_configured: true,
        ..Default::default()
    };
    let sender = make_person(&pool).await;
    let gil = format!("{}@customer.test", u("gil"));
    let cid = campaign(
        &pool,
        sender,
        &[list_with(&pool, &[(&gil, "Gil", "Eta")]).await],
        None,
        true,
    )
    .await;
    process_once(&pool, cid, &smtp).await;
    let ((status, sent_at, _), c) = queue_state(&pool, cid).await;
    assert!(
        status == "pending" && sent_at.is_none(),
        "nothing went out: it waits for SMTP"
    );
    assert_eq!(c.sent_count.unwrap_or(0), 0);
    assert_eq!(c.status.as_deref(), Some("sending"));
}

#[tokio::test]
async fn test_a_block_that_cannot_render_fails_its_email_instead_of_retrying_forever() {
    let pool = setup("render_fail").await;
    let smtp = FakeSmtp::default();
    let sender = make_person(&pool).await;
    let hal = format!("{}@customer.test", u("hal"));
    let cid = campaign_with(
        &pool,
        sender,
        &[list_with(&pool, &[(&hal, "Hal", "Theta")]).await],
        None,
        true,
        json!([{"type": "spacer", "height": "abc"}]),
    )
    .await;
    process_once(&pool, cid, &smtp).await;
    let ((status, _, err), c) = queue_state(&pool, cid).await;
    let err = err.unwrap_or_default();
    assert!(
        status == "failed" && err.contains("could not be rendered") && err.contains("abc"),
        "{err}"
    );
    assert_eq!(
        err,
        "Email could not be rendered: invalid literal for int() with base 10: 'abc'"
    );
    assert_eq!(c.failed_count, Some(1));
    assert_eq!(
        c.status.as_deref(),
        Some("sent"),
        "nothing left pending: the campaign closes"
    );
    assert!(smtp.sent().is_empty());
}

#[test]
fn test_the_button_colour_must_be_a_css_colour() {
    for (color, rendered) in [
        (json!("#1a2b3c"), "#1a2b3c"),
        (json!("#fff"), "#fff"),
        (json!("rgb(10, 20, 30)"), "rgb(10, 20, 30)"),
        (json!("teal"), "teal"),
        (Value::Null, "#1f4e79"),
        (json!("red;\" onmouseover=\"alert(1)"), "#1f4e79"),
        (json!("red;background:url(x)"), "#1f4e79"),
        (json!("</a><script>x</script>"), "#1f4e79"),
    ] {
        let html = em::_render_block(
            &json!({"type": "cta_button", "label": "Go", "button_url": "https://x.test", "button_color": color}),
            &Default::default(),
        )
        .unwrap();
        assert!(
            html.contains(&format!("background:{rendered};color:#ffffff;")),
            "{html}"
        );
        assert!(
            !html.contains("<script>") && !html.contains("onmouseover") && !html.contains("url(x)")
        );
    }
}

// ─── Preview ──────────────────────────────────────────────────────────────────

/// What POST /marketing/email/preview does with these service functions.
#[tokio::test]
async fn test_preview_renders_the_whole_email_for_a_list_contact_and_sends_nothing() {
    let pool = setup("preview").await;
    signature_template(&pool).await;
    let sender = make_person(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let preview = SignatureCampaign {
        id: "preview".into(),
        include_signature: true,
        sender_user_id: Some(sender),
        created_by_id: Some(sender),
    };
    let signature = em::sender_signature(&mut conn, Some(&preview))
        .await
        .unwrap();
    assert!(signature.contains("class=\"sig\""));
    let ctx = em::recipient_context(
        &mut conn,
        "fay@customer.test",
        Some("Fay"),
        Some("Fay Tester"),
        Some("Zeta Corp"),
        "#",
    )
    .await
    .unwrap();
    assert_eq!(em::apply_tokens("Hi {{first_name}}", &ctx), "Hi Fay");
    let html = em::render_campaign_html(
        blocks().as_array().unwrap(),
        &ctx,
        "preview",
        &signature,
        true,
    )
    .unwrap();
    assert!(html.contains("News for Zeta Corp") && html.contains("class=\"sig\""));
    assert!(!html.contains("/track/") && !html.contains("/unsubscribe?token="));

    let none = SignatureCampaign {
        include_signature: false,
        ..preview.clone()
    };
    assert_eq!(
        em::sender_signature(&mut conn, Some(&none)).await.unwrap(),
        ""
    );
    assert_eq!(em::sender_signature(&mut conn, None).await.unwrap(), "");
    let nobody = SignatureCampaign {
        sender_user_id: None,
        created_by_id: None,
        ..preview
    };
    assert_eq!(
        em::sender_signature(&mut conn, Some(&nobody))
            .await
            .unwrap(),
        ""
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM email_queue_items")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

// ─── M53: marketing queue worker ─────────────────────────────────────────────

async fn settings_rows(pool: &PgPool, rows: &[(&str, &str)]) {
    for (k, v) in rows {
        sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
            .bind(k)
            .bind(v)
            .execute(pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn test_queue_worker_settings_are_validated() {
    let pool = setup("settings").await;
    settings_rows(
        &pool,
        &[
            ("marketing.email_daily_limit", "100"),
            ("marketing.email_min_delay_seconds", "30"),
            ("marketing.email_max_delay_seconds", "60"),
            ("marketing.queue_lock_ttl_seconds", "60"),
        ],
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    match em::worker_settings(&mut conn).await {
        Err(granary_core::settings_store::StoreError::Setting(e)) => {
            assert!(
                e.detail.contains("marketing.queue_lock_ttl_seconds"),
                "{}",
                e.detail
            );
            assert_eq!(
                e.detail,
                "Setting 'marketing.queue_lock_ttl_seconds' must be longer than marketing.email_max_delay_seconds. \
                 Set it in Settings \u{2192} Email & WhatsApp \u{2192} Email campaigns."
            );
        }
        other => panic!("{other:?}"),
    }
    settings_rows(&pool, &[("marketing.email_max_delay_seconds", "20")]).await;
    match em::worker_settings(&mut conn).await {
        Err(granary_core::settings_store::StoreError::Setting(e)) => {
            assert!(
                e.detail
                    .contains("is below marketing.email_min_delay_seconds"),
                "{}",
                e.detail
            )
        }
        other => panic!("{other:?}"),
    }
    settings_rows(&pool, &[("marketing.email_max_delay_seconds", "59")]).await;
    let cfg = em::worker_settings(&mut conn).await.unwrap();
    assert_eq!(
        (cfg.daily_limit, cfg.min_delay, cfg.max_delay, cfg.lock_ttl),
        (100, 30, 59, 60)
    );
}

#[tokio::test]
async fn test_the_daily_cap_counts_the_company_local_day() {
    use chrono::{TimeZone, Utc};
    let pool = setup("daily_cap").await;
    settings_rows(&pool, &[("timezone", "Asia/Tokyo")]).await;
    let tz = chrono_tz::Asia::Tokyo;
    let today = granary_core::clock::today_in(tz);
    let midnight = tz
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let mut conn = pool.acquire().await.unwrap();
    let before = em::_sent_today(&mut conn).await.unwrap();
    let sender = make_person(&pool).await;
    let ml = list_with(&pool, &[("a@example.test", "A", "X")]).await;
    let cid = campaign(&pool, sender, &[], None, true).await;
    let contact: Uuid =
        sqlx::query_scalar("SELECT id FROM mailing_list_contacts WHERE list_id = $1")
            .bind(ml)
            .fetch_one(&pool)
            .await
            .unwrap();
    for (email, at) in [
        ("a@example.test", midnight + chrono::Duration::minutes(1)),
        ("b@example.test", midnight - chrono::Duration::minutes(1)),
    ] {
        sqlx::query(
            "INSERT INTO email_queue_items (id, campaign_id, list_id, contact_id, email, status, tracking_id, sent_at) \
             VALUES ($1, $2, $3, $4, $5, 'sent', $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(cid)
        .bind(ml)
        .bind(contact)
        .bind(email)
        .bind(Uuid::new_v4().simple().to_string())
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    }
    assert_eq!(em::_sent_today(&mut conn).await.unwrap(), before + 1);
}

static SENDERS: Mutex<Vec<tokio::task::Id>> = Mutex::new(Vec::new());
/// The lease key is one Redis key for every test: the worker tests take turns.
static LEASE_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn fake_process(
    _db: &mut PgConnection,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + Send + '_>> {
    Box::pin(async {
        SENDERS.lock().unwrap().push(tokio::task::id());
        Ok(false)
    })
}

fn senders() -> Vec<tokio::task::Id> {
    SENDERS.lock().unwrap().clone()
}

/// A fresh database with the worker settings, no lease in Redis and no recorded sends.
async fn worker_setup(
    tag: &str,
    min_delay: &str,
    max_delay: &str,
    ttl: &str,
) -> (PgPool, redis::aio::ConnectionManager) {
    use redis::AsyncCommands;
    let pool = setup(tag).await;
    settings_rows(
        &pool,
        &[
            ("marketing.email_daily_limit", "100"),
            ("marketing.email_min_delay_seconds", min_delay),
            ("marketing.email_max_delay_seconds", max_delay),
            ("marketing.queue_lock_ttl_seconds", ttl),
        ],
    )
    .await;
    let mut r = granary_core::redis_client::get_redis()
        .conn()
        .await
        .unwrap();
    let _: () = r.del(em::LOCK_KEY).await.unwrap();
    SENDERS.lock().unwrap().clear();
    (pool, r)
}

/// Waits (up to `secs`) until `id` has processed an item.
async fn wait_for_send(id: tokio::task::Id, secs: u64) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < until {
        if senders().contains(&id) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_only_one_queue_worker_sends() {
    use redis::AsyncCommands;
    let _turn = LEASE_TESTS.lock().await;
    let (pool, mut r) = worker_setup("one_worker", "1", "1", "5").await;
    let (stop, rx) = tokio::sync::watch::channel(false);
    let workers: Vec<_> = (0..3)
        .map(|_| {
            tokio::spawn(em::queue_worker_with(
                pool.clone(),
                rx.clone(),
                fake_process,
            ))
        })
        .collect();
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    stop.send(true).unwrap();
    for w in workers {
        w.await.unwrap();
    }
    let senders = senders();
    let distinct: std::collections::HashSet<_> = senders.iter().collect();
    assert!(!senders.is_empty() && distinct.len() == 1, "{senders:?}");
    let held: Option<String> = r.get(em::LOCK_KEY).await.unwrap();
    assert_eq!(held, None, "released on shutdown");
}

/// A worker that dies without its `finally` (a killed process) keeps the lease until the TTL runs out;
/// then a waiting worker takes it over and sends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_the_lease_is_taken_over_after_the_holder_crashes() {
    let _turn = LEASE_TESTS.lock().await;
    let (pool, mut r) = worker_setup("takeover", "1", "1", "3").await;
    let (_stop, rx) = tokio::sync::watch::channel(false);
    let a = tokio::spawn(em::queue_worker_with(
        pool.clone(),
        rx.clone(),
        fake_process,
    ));
    let a_id = a.id();
    assert!(wait_for_send(a_id, 10).await, "first worker never sent");
    a.abort();
    assert!(a.await.unwrap_err().is_cancelled());
    let left_ms: i64 = redis::cmd("PTTL")
        .arg(em::LOCK_KEY)
        .query_async(&mut r)
        .await
        .unwrap();
    assert!(
        left_ms > 500,
        "the crashed holder's lease stays: {left_ms} ms"
    );
    SENDERS.lock().unwrap().clear();

    let b = tokio::spawn(em::queue_worker_with(
        pool.clone(),
        rx.clone(),
        fake_process,
    ));
    let b_id = b.id();
    tokio::time::sleep(std::time::Duration::from_millis((left_ms - 300) as u64)).await;
    assert!(
        senders().is_empty(),
        "sent while the lease was held: {:?}",
        senders()
    );
    assert!(wait_for_send(b_id, 4).await, "the lease was not taken over");
    assert!(senders().iter().all(|id| *id == b_id), "{:?}", senders());
    b.abort();
    let _ = b.await;
    let _: () = redis::cmd("DEL")
        .arg(em::LOCK_KEY)
        .query_async(&mut r)
        .await
        .unwrap();
}

/// A holder whose lease expired and was taken by another process stops sending (the token-checked
/// renew fails), and its shutdown leaves the new holder's lease alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_a_worker_that_lost_the_lease_stops_sending_and_keeps_the_new_lease() {
    use redis::AsyncCommands;
    let _turn = LEASE_TESTS.lock().await;
    let (pool, mut r) = worker_setup("lost_lease", "1", "1", "5").await;
    let (stop, rx) = tokio::sync::watch::channel(false);
    let a = tokio::spawn(em::queue_worker_with(pool.clone(), rx, fake_process));
    assert!(wait_for_send(a.id(), 10).await, "worker never sent");
    let _: () = r.set_ex(em::LOCK_KEY, "another-process", 30).await.unwrap();
    // Let an iteration that was already past its renew finish.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    SENDERS.lock().unwrap().clear();
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(
        senders().is_empty(),
        "sent without the lease: {:?}",
        senders()
    );
    stop.send(true).unwrap();
    a.await.unwrap();
    let held: Option<String> = r.get(em::LOCK_KEY).await.unwrap();
    assert_eq!(held.as_deref(), Some("another-process"));
    let _: () = r.del(em::LOCK_KEY).await.unwrap();
}

/// Shutdown (Python's task.cancel() in the lifespan) interrupts the pause between sends at once and
/// releases the lease, so another process can take over without waiting for the TTL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_shutdown_during_the_pause_releases_the_lease_at_once() {
    use redis::AsyncCommands;
    let _turn = LEASE_TESTS.lock().await;
    let (pool, mut r) = worker_setup("shutdown", "30", "30", "60").await;
    let (stop, rx) = tokio::sync::watch::channel(false);
    let a = tokio::spawn(em::queue_worker_with(pool.clone(), rx, fake_process));
    assert!(wait_for_send(a.id(), 10).await, "worker never sent");
    let held: Option<String> = r.get(em::LOCK_KEY).await.unwrap();
    assert!(held.is_some(), "held while pausing");
    stop.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), a)
        .await
        .expect("the worker stops during its pause")
        .unwrap();
    let held: Option<String> = r.get(em::LOCK_KEY).await.unwrap();
    assert_eq!(held, None, "released on shutdown");
}

// ─── dispatch_campaign details ───────────────────────────────────────────────

#[tokio::test]
async fn dispatch_dedupes_by_email_skips_deleted_lists_and_schedules() {
    let pool = setup("dispatch").await;
    let sender = make_person(&pool).await;
    let a = list_with(
        &pool,
        &[("dup@x.test", "Dup", "A"), ("one@x.test", "One", "A")],
    )
    .await;
    let b = list_with(&pool, &[("DUP@x.test", "Dup2", "B")]).await;
    let gone = list_with(&pool, &[("gone@x.test", "Gone", "G")]).await;
    sqlx::query("UPDATE mailing_lists SET is_deleted = true WHERE id = $1")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE mailing_list_contacts SET is_subscribed = false WHERE email = 'one@x.test'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let cid = campaign(&pool, sender, &[a, b, gone], None, true).await;
    let mut c = load_campaign(&pool, cid).await;
    let at = chrono::Utc::now() + chrono::Duration::days(1);
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        em::dispatch_campaign(&mut c, &mut conn, Some(at))
            .await
            .unwrap(),
        1
    );
    let stored = load_campaign(&pool, cid).await;
    assert_eq!(stored.status.as_deref(), Some("scheduled"));
    assert_eq!(stored.total_recipients, Some(1));
    assert_eq!(
        stored.scheduled_at.map(|t| t.timestamp_micros()),
        Some(at.timestamp_micros())
    );
    let (full, status): (Option<String>, String) =
        sqlx::query_as("SELECT full_name, status FROM email_queue_items WHERE campaign_id = $1")
            .bind(cid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (full.as_deref(), status.as_str()),
        (Some("Dup Tester"), "pending")
    );

    // A scheduled campaign is not sent before its time.
    let smtp = FakeSmtp::default();
    assert!(!em::_process_next_with(&mut conn, &smtp).await.unwrap());
    // No lists: nothing happens.
    let empty = campaign(&pool, sender, &[], None, true).await;
    let mut c = load_campaign(&pool, empty).await;
    assert_eq!(
        em::dispatch_campaign(&mut c, &mut conn, None)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        load_campaign(&pool, empty).await.status.as_deref(),
        Some("draft")
    );
}

/// A send that raises marks the item failed with the error text and counts it.
#[tokio::test]
async fn a_failed_send_is_recorded() {
    struct Failing;
    #[async_trait::async_trait]
    impl MailBackend for Failing {
        async fn load_crm_smtp(&self, db: &mut PgConnection) -> anyhow::Result<SmtpCfg> {
            FakeSmtp::default().load_crm_smtp(db).await
        }
        async fn send_via_config(
            &self,
            _: &SmtpCfg,
            _: &str,
            _: &str,
            _: &str,
            _: &mut PgConnection,
            _: &[(String, String)],
        ) -> anyhow::Result<bool> {
            Err(anyhow::anyhow!("{}", "x".repeat(600)))
        }
        async fn sent_today(&self, _db: &mut PgConnection) -> anyhow::Result<i64> {
            Ok(0)
        }
    }
    let pool = setup("failed").await;
    let sender = make_person(&pool).await;
    let cid = campaign(
        &pool,
        sender,
        &[list_with(&pool, &[("f@x.test", "F", "X")]).await],
        None,
        true,
    )
    .await;
    dispatch(&pool, cid).await;
    let mut conn = pool.acquire().await.unwrap();
    assert!(em::_process_next_with(&mut conn, &Failing).await.unwrap());
    let (status, err): (String, Option<String>) = sqlx::query_as(
        "SELECT status, error_message FROM email_queue_items WHERE campaign_id = $1",
    )
    .bind(cid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(err.unwrap().len(), 500);
    let c = load_campaign(&pool, cid).await;
    assert_eq!(
        (c.failed_count, c.sent_count, c.status.as_deref()),
        (Some(1), Some(0), Some("sent"))
    );
}
