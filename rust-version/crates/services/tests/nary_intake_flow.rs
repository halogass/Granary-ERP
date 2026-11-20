//! Ports of tests/test_nary_p2_web3forms.py, tests/test_nary_email_intake.py and the triage tests of
//! tests/test_nary_p2_identity.py, on the brain seam (ai::registry::seam). The poller is the real
//! crm_inbox::_ingest with its nary calls pointed at the real functions (NaryCalls), and Nary's queue
//! is the real events::dispatch_pending with every module's subscribers (events::module_subscriptions). Each test gets its own database; the brain's seam lock runs the
//! pipeline tests one at a time (process-wide pools, hooks and seams).

mod support;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

use granary_db::rows::crm::{Contact, Lead};
use granary_db::rows::crm_inbox::CrmEmailMessage;
use granary_db::rows::nary::{AgentPendingAction, NaryExpectedArrival, NaryTriageVerdict};
use granary_services::ai::base::ChatResult;
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use granary_services::crm_inbox::{self, NaryCalls};
use granary_services::email::{self as email_service, Seams, SendCall, SmtpConfig};
use granary_services::imap_sync::pyemail::{message_from_bytes, utils::parseaddr, Message};
use granary_services::nary::{self, email_intake, events, intake, triage, web3forms};

const MAILBOX: &str = "hello@example.com";
const KNOWN: [&str; 11] = [
    "name",
    "email",
    "phone",
    "company",
    "interested_in",
    "message",
    "from_page",
    "submission_source",
    "timezone",
    "landing_page",
    "first_referrer",
];

fn routes() -> Value {
    json!({"web3forms": "email", "inline_web3forms": "email", "whatsapp": "whatsapp"})
}

fn known() -> Vec<String> {
    KNOWN.iter().map(|s| s.to_string()).collect()
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/web3forms")
}

fn load(name: &str) -> (Vec<u8>, Message) {
    let raw = std::fs::read(fixtures().join(name)).unwrap();
    let msg = message_from_bytes(&raw).unwrap();
    (raw, msg)
}

fn eml_names() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".eml"))
        .collect();
    v.sort();
    v
}

/// `msg.get_body(preferencelist=("plain",)).get_content()` for these fixtures (multipart/alternative or a
/// single text part): the first text/plain part.
fn plain(msg: &Message) -> String {
    for part in msg.walk() {
        if part.get_content_type().unwrap() == "text/plain" {
            if let granary_services::imap_sync::pyemail::Content::Text(t) =
                part.get_content().unwrap()
            {
                return t;
            }
        }
    }
    panic!("no text/plain part")
}

fn plain_part(msg: &Message) -> &Message {
    msg.walk()
        .into_iter()
        .find(|p| p.get_content_type().unwrap() == "text/plain")
        .unwrap()
}

// ─── Parser on the real samples ───────────────────────────────────────────────

#[test]
fn test_email_journey_forms_parse_every_field() {
    let (_, msg) = load("web3forms_laptops.eml");
    let p = web3forms::parse_text(Some(&plain(&msg)));
    let want: Vec<(String, String)> = [
        ("name", "jane doe"),
        ("email", "jane.doe.lead@example.org"),
        ("phone", "+442079460123"),
        ("company", "Northwind Traders"),
        ("interested_in", "laptops"),
        ("message", "need a quote for 20 laptops"),
        ("from_page", "https://www.example.com/products/laptops/"),
        ("submission_source", "web3forms"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(p.fields, want);
    assert!(!p.loose);

    let (_, msg) = load("inline_web3forms_support.eml");
    let p = web3forms::parse_text(Some(&plain(&msg)));
    assert_eq!(p.get("submission_source"), "inline_web3forms");
    assert_eq!(p.get("phone"), ""); // an empty field stays empty
    assert_eq!(p.get("company"), "Contoso Copy & Print Center");
    assert!(
        p.get("message").starts_with("Use our existing laptops")
            && p.get("message").ends_with("100 branches")
    );
}

#[test]
fn test_a_many_line_quoted_printable_message_is_kept_whole() {
    let (_, msg) = load("whatsapp_multiline.eml");
    assert_eq!(
        plain_part(&msg)
            .get("Content-Transfer-Encoding")
            .unwrap()
            .as_deref(),
        Some("quoted-printable")
    );
    let p = web3forms::parse_text(Some(&plain(&msg)));
    let message = p.get("message");
    let lines: Vec<&str> = message.split('\n').collect();
    assert_eq!(
        lines[0],
        "We need laptops for a new office of 120 staff and one support plan for all of them"
    );
    assert!(lines.contains(&"1-Hardware") && *lines.last().unwrap() == "Monthly usage report");
    assert!(lines.len() > 25);
    assert_eq!(p.get("submission_source"), "whatsapp");
    assert_eq!(p.get("timezone"), "Asia/Riyadh");
    assert!(p.get("landing_page").ends_with("?utm_source=chatgpt.com"));
    assert_eq!(p.get("first_referrer"), "https://chatgpt.com/");
    let order: BTreeSet<&String> = p.order.iter().collect();
    let k = known();
    assert!(p.order == k || order == k.iter().collect());
}

#[test]
fn test_a_colon_inside_the_message_does_not_start_a_new_field() {
    let (_, msg) = load("whatsapp_no_email.eml");
    let p = web3forms::parse_text(Some(&plain(&msg)));
    assert_eq!(p.get("email"), "");
    assert!(p.get("message").contains("WhatsApp: +639171234567"));
    assert!(p
        .get("message")
        .contains("\n\nWould your team be available"));
    assert_eq!(p.get("first_referrer"), "(direct)");
    assert!(!p
        .fields
        .iter()
        .any(|(k, _)| k == "whatsapp" || k == "best_regards"));
}

#[test]
fn test_checks_pass_on_every_real_sample() {
    for f in eml_names() {
        let (_, msg) = load(&f);
        let p = web3forms::parse_text(Some(&plain(&msg)));
        let rt = msg.get("Reply-To").unwrap().filter(|s| !s.is_empty());
        let chk = web3forms::check(
            &p,
            rt.as_deref(),
            &known(),
            &sv(&["name", "submission_source"]),
            &routes(),
        );
        assert!(chk.problems.is_empty(), "{f} {:?}", chk.problems);
    }
}

#[test]
fn test_format_drift_is_reported() {
    let drifted = "Name: Bob\nEmail: bob@x.co\nsubmission_source: web3forms\nbudget  : 5000\n";
    let p = web3forms::parse_text(Some(drifted));
    let chk = web3forms::check(
        &p,
        Some("other@x.co"),
        &known(),
        &sv(&["name", "submission_source"]),
        &routes(),
    );
    assert!(chk.problems.join("; ").contains("new field(s): budget"));
    assert!(chk.problems.iter().any(|x| x.contains("missing field")));
    let loose = web3forms::parse_text(Some("name: Bob\nsubmission_source: carrier_pigeon\n"));
    let chk = web3forms::check(&loose, None, &known(), &sv(&["name"]), &routes());
    assert!(loose.loose && chk.problems.iter().any(|x| x.contains("layout")));
    assert!(chk.problems.iter().any(|x| x.contains("carrier_pigeon")));
}

#[test]
fn test_reply_to_mismatch_is_flagged() {
    let p = web3forms::parse_text(Some(
        "name  : A\nemail  : a@corp.com\nsubmission_source  : web3forms\n",
    ));
    let chk = web3forms::check(&p, Some("b@corp.com"), &known(), &sv(&["name"]), &routes());
    assert!(chk.reply_to_mismatch);
}

#[test]
fn test_detection_uses_sender_and_dkim_not_the_subject() {
    let w = sv(&["web3forms.com"]);
    for f in eml_names() {
        let (_, msg) = load(&f);
        let auth = web3forms::header_auth(&msg);
        assert!(auth["dkim_domains"]
            .as_array()
            .unwrap()
            .contains(&json!("web3forms.com")));
        let from = parseaddr(&msg.get("From").unwrap().unwrap()).1;
        let found = web3forms::detect(&from, Some(&auth), &w, &w);
        assert_eq!(
            found,
            Some(json!({"signed_by": ["web3forms.com"], "verified": false})),
            "{f}"
        );
    }
    assert_eq!(
        web3forms::detect(
            "notify@web3forms.evil.io",
            Some(&json!({"dkim_domains": ["web3forms.com"]})),
            &w,
            &w
        ),
        None
    );
    assert_eq!(
        web3forms::detect(
            "notify+abc@web3forms.com",
            Some(&json!({"dkim_domains": ["amazonses.com"]})),
            &w,
            &w
        ),
        None
    );
    let failed = json!({"dkim_domains": ["web3forms.com"], "dkim_results": [{"dkim": "fail", "domain": "web3forms.com"}]});
    assert_eq!(
        web3forms::detect("notify+abc@web3forms.com", Some(&failed), &w, &w),
        None
    );
    let passed = json!({"dkim_domains": ["web3forms.com"], "dkim_results": [{"dkim": "pass", "domain": "web3forms.com"}]});
    assert_eq!(
        web3forms::detect("notify+abc@web3forms.com", Some(&passed), &w, &w).unwrap()["verified"],
        json!(true)
    );
}

#[test]
fn test_authentication_results_are_read() {
    let raw = b"From: Web Form <notify+x@web3forms.com>\r\nDKIM-Signature: v=1; a=rsa-sha256; d=web3forms.com; s=x; b=y\r\n\
Authentication-Results: mx.example.com; dkim=pass (2048-bit key) header.d=web3forms.com header.i=@web3forms.com\r\n\
Subject: whatever\r\n\r\nname  : A\r\n";
    let auth = web3forms::header_auth(&message_from_bytes(raw).unwrap());
    assert_eq!(
        auth,
        json!({"dkim_domains": ["web3forms.com"], "dkim_results": [{"dkim": "pass", "domain": "web3forms.com"}]})
    );
}

// ─── email_intake: what is skipped before any AI call ─────────────────────────

fn rules() -> Value {
    json!({"list_headers": ["List-Unsubscribe", "List-Id"], "precedence": ["bulk", "list"], "auto_submitted": true,
           "sender_prefixes": ["noreply", "newsletter"], "sender_domains": ["mailchimp.com"]})
}

fn own() -> BTreeSet<String> {
    ["example.com".to_string()].into_iter().collect()
}

#[test]
fn test_newsletters_lists_and_automated_mail_are_skipped() {
    let cases: Vec<(Vec<(&str, &str)>, &str, &str)> = vec![
        (
            vec![("List-Unsubscribe", "<mailto:x@y>")],
            "news@shop.com",
            "mailing list",
        ),
        (vec![("Precedence", "bulk")], "a@shop.com", "Precedence"),
        (
            vec![("Auto-Submitted", "auto-replied")],
            "a@shop.com",
            "Auto-Submitted",
        ),
        (vec![], "noreply@shop.com", "automated sender"),
        (vec![], "newsletter+id@shop.com", "automated sender"),
        (vec![], "a@us1.mailchimp.com", "skipped domain"),
        (vec![], "bob@example.com", "our own domain"),
    ];
    for (headers, addr, why) in cases {
        let h: HashMap<String, String> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let got = email_intake::skip_reason(&h, addr, &rules(), &own())
            .unwrap()
            .unwrap_or_default();
        assert!(got.contains(why), "{addr}: {got}");
    }
}

#[test]
fn test_a_person_writing_is_not_skipped() {
    let h: HashMap<String, String> = [("Auto-Submitted".to_string(), "no".to_string())]
        .into_iter()
        .collect();
    assert_eq!(
        email_intake::skip_reason(&h, "bob.newsroom@vandelay.example", &rules(), &own()).unwrap(),
        None
    );
    assert_eq!(
        email_intake::skip_reason(
            &HashMap::new(),
            "noreplyfan@vandelay.example",
            &rules(),
            &BTreeSet::new()
        )
        .unwrap(),
        None
    );
}

#[test]
fn test_only_the_senders_own_words_are_read() {
    let body = "Please send me your prices.\nMy WhatsApp is 090 3456 7890\n\nOn Mon, 1 Oct 2026 ACME wrote:\n> Our number +81 90 1234 5678";
    assert_eq!(
        email_intake::customer_text(Some(body)),
        "Please send me your prices.\nMy WhatsApp is 090 3456 7890"
    );
}

// ─── The environment of the pipeline tests ────────────────────────────────────

struct Env {
    pool: PgPool,
    brain: Brain,
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
        // intake.form / email.new (and every other Nary subscriber) are in events::module_subscriptions.
        granary_services::approvals::notify::set_external_delivery_enabled(false);
    });
}

async fn setup(tag: &str) -> Env {
    let brain = Brain::p2();
    let seam = brain.install().await; // the seam lock: one pipeline test at a time
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
    working_hours(&mut c, true).await;
    Env {
        pool,
        brain,
        _seam: seam,
        sent,
    }
}

async fn conn(env: &Env) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    env.pool.acquire().await.unwrap()
}

async fn set(db: &mut PgConnection, k: &str, v: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(k)
        .bind(v)
        .execute(&mut *db)
        .await
        .unwrap();
    assert_eq!(n.rows_affected(), 1, "setting not seeded: {k}");
}

/// `working_hours(True)`: the setting covers the whole week (Python monkeypatches rhythm.is_working_time).
async fn working_hours(db: &mut PgConnection, inside: bool) {
    let wh = if inside {
        json!({"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "23:59"})
    } else {
        json!({"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "00:00"})
    };
    set(db, "nary.working_hours", &wh.to_string()).await;
}

async fn owner(db: &mut PgConnection) -> Uuid {
    let user: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(user)
    .execute(&mut *db)
    .await
    .unwrap();
    user
}

async fn ingest(env: &Env, raw: &[u8], uid: i64, folder: &str) -> bool {
    let mut c = conn(env).await;
    let mut tx = c.begin().await.unwrap();
    let ok = crm_inbox::_ingest(&mut tx, raw, uid, folder, false, MAILBOX)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    ok
}

async fn last_message(env: &Env) -> CrmEmailMessage {
    let mut c = conn(env).await;
    sqlx::query_as("SELECT * FROM crm_email_messages ORDER BY created_at DESC LIMIT 1")
        .fetch_one(&mut *c)
        .await
        .unwrap()
}

async fn dispatch(env: &Env) -> (i64, i64) {
    let (mut d, mut f) = (0, 0);
    for _ in 0..10 {
        let out = events::dispatch_pending(Some(&env.pool)).await.unwrap();
        let (od, of) = (
            out["dispatched"].as_i64().unwrap_or(0),
            out["failed"].as_i64().unwrap_or(0),
        );
        d += od;
        f += of;
        if od == 0 && of == 0 {
            break;
        }
    }
    (d, f)
}

async fn cards(env: &Env, action: &str) -> Vec<AgentPendingAction> {
    let mut c = conn(env).await;
    sqlx::query_as("SELECT * FROM agent_pending_actions WHERE action = $1 ORDER BY created_at")
        .bind(action)
        .fetch_all(&mut *c)
        .await
        .unwrap()
}

async fn message(env: &Env, id: Uuid) -> CrmEmailMessage {
    let mut c = conn(env).await;
    sqlx::query_as("SELECT * FROM crm_email_messages WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}

async fn lead_for(env: &Env, msg_id: Uuid) -> Lead {
    let msg = message(env, msg_id).await;
    let lead_id =
        Uuid::parse_str(msg.auth.unwrap()["web3forms"]["lead_id"].as_str().unwrap()).unwrap();
    let mut c = conn(env).await;
    sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(lead_id)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}

async fn contact(env: &Env, id: Uuid) -> Contact {
    let mut c = conn(env).await;
    sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}

async fn thread_links(env: &Env, id: Uuid) -> (Option<Uuid>, Option<Uuid>) {
    let mut c = conn(env).await;
    sqlx::query_as("SELECT linked_contact_id, linked_lead_id FROM crm_email_threads WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}

async fn events_of(env: &Env, t: &str) -> Vec<(Option<Uuid>,)> {
    let mut c = conn(env).await;
    sqlx::query_as("SELECT entity_id FROM domain_events WHERE type = $1")
        .bind(t)
        .fetch_all(&mut *c)
        .await
        .unwrap()
}

// ─── Triage (tests/test_nary_p2_identity.py) ──────────────────────────────────

#[tokio::test]
async fn test_a_spam_verdict_on_business_words_goes_to_the_second_model() {
    let env = setup("n7_triage_cascade").await;
    env.brain.set(
        "nary.triage",
        Answer::json(json!({"verdict": "spam", "language": "en", "reason": "looks like promo"})),
    );
    env.brain.set(
        "nary.cascade_review",
        Answer::json(
            json!({"verdict": "prospect", "language": "en", "reason": "asks for a quotation"}),
        ),
    );
    let mut c = conn(&env).await;
    let v = triage::classify(
        &mut c,
        "Please send a price quotation for 5 laptops",
        None,
        "",
    )
    .await
    .unwrap();
    assert!(v.verdict == "prospect" && v.cascaded && v.cascade_reason.unwrap().contains("price"));
    assert_eq!(env.brain.jobs(), vec!["nary.triage", "nary.cascade_review"]);
}

#[tokio::test]
async fn test_spam_without_business_words_stays_with_the_first_model() {
    let env = setup("n7_triage_spam").await;
    env.brain.set(
        "nary.triage",
        Answer::json(json!({"verdict": "spam", "language": "en", "reason": "crypto"})),
    );
    let mut c = conn(&env).await;
    let v = triage::classify(&mut c, "Earn 500% with our crypto bot!!!", None, "")
        .await
        .unwrap();
    assert!(v.verdict == "spam" && !v.cascaded);
    assert_eq!(env.brain.jobs(), vec!["nary.triage"]);
}

#[tokio::test]
async fn test_a_refusal_or_garbage_escalates() {
    let env = setup("n7_triage_refusal").await;
    let mut refusal = ChatResult::text("");
    refusal.stop_reason = "refusal".into();
    env.brain.set("nary.triage", Answer::result(refusal));
    let mut c = conn(&env).await;
    let v = triage::classify(&mut c, "hmm", None, "").await.unwrap();
    assert!(v.cascaded);
    assert_eq!(env.brain.jobs(), vec!["nary.triage", "nary.cascade_review"]);
    env.brain.clear();
    env.brain.set("nary.triage", Answer::text("not json"));
    let v = triage::classify(&mut c, "hmm", None, "").await.unwrap();
    assert!(
        v.cascaded && v.cascade_reason.as_deref() == Some("the first answer could not be read")
    );
}

#[tokio::test]
async fn test_owner_corrections_become_examples() {
    let env = setup("n7_triage_examples").await;
    let mut c = conn(&env).await;
    let text = "We are a fibre cable vendor and would like to offer a partnership";
    let v = triage::classify(&mut c, text, None, "").await.unwrap();
    let row = triage::record(&mut c, "wa_chat", "x@c.us", &v, text, None)
        .await
        .unwrap();
    let admin: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *c)
            .await
            .unwrap();
    let user = nary::roles::load_actor(&mut c, admin)
        .await
        .unwrap()
        .unwrap();
    triage::correct(&mut c, row.id, "vendor", &user, Some("they sell to us"))
        .await
        .unwrap();
    env.brain.clear();
    triage::classify(&mut c, "We are a cable distributor", None, "")
        .await
        .unwrap();
    let calls = env.brain.calls();
    let system = calls[0].1.chat().unwrap().system.clone();
    assert!(
        system.contains("CORRECTIONS THE Owner MADE")
            && system.contains("→ vendor (the model had said prospect)")
    );
    assert!(triage::correct(&mut c, row.id, "nonsense", &user, None)
        .await
        .is_err());
}

// ─── The poller → intake → lead (tests/test_nary_p2_web3forms.py) ─────────────

#[tokio::test]
async fn test_an_email_journey_form_becomes_a_lead_and_is_acknowledged_by_new_email() {
    let env = setup("n7_w3f_email_journey").await;
    {
        let mut c = conn(&env).await;
        set(
            &mut c,
            "nary.web3forms.interest_map",
            r#"{"laptops": "Laptops", "general": null}"#,
        )
        .await;
    }
    let (raw, _) = load("web3forms_laptops.eml");
    assert!(ingest(&env, &raw, 1, "INBOX").await);
    let msg = last_message(&env).await;
    assert_eq!(msg.reply_to.as_deref(), Some("jane.doe.lead@example.org"));
    assert_eq!(thread_links(&env, msg.thread_id).await.0, None); // never auto-linked to the notify sender
    let ev = events_of(&env, "intake.form").await;
    assert_eq!(ev, vec![(Some(msg.id),)]);

    let mut c = conn(&env).await;
    set(&mut c, "nary.ack.enabled", "true").await;
    let (_, failed) = dispatch(&env).await;
    assert_eq!(failed, 0);
    let lead = lead_for(&env, msg.id).await;
    assert!(lead.source == "web3forms" && lead.status.as_deref() == Some("qualifying"));
    assert_eq!(lead.interested_products, Some(vec!["Laptops".to_string()]));
    assert_eq!(lead.currency.as_deref(), Some("USD")); // +44: not the home country
    let raw_data = lead.raw_channel_data.clone().unwrap();
    assert_eq!(raw_data["web3forms"]["fields"]["interested_in"], "laptops");
    assert_eq!(raw_data["locale"]["country"], "GB");
    let person = contact(&env, lead.contact_id.unwrap()).await;
    assert_eq!(person.email.as_deref(), Some("jane.doe.lead@example.org"));
    assert_eq!(person.whatsapp.as_deref(), Some("+442079460123"));
    assert_eq!(
        thread_links(&env, msg.thread_id).await,
        (Some(person.id), Some(lead.id))
    );
    let verdict: NaryTriageVerdict =
        sqlx::query_as("SELECT * FROM nary_triage_verdicts WHERE lead_id = $1")
            .bind(lead.id)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(verdict.verdict, "prospect");
    // The acknowledgement: a new email to the lead, never "Re: New Lead [...]".
    let sent = env.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, vec!["jane.doe.lead@example.org".to_string()]);
    assert!(
        !sent[0].subject.to_lowercase().starts_with("re:") && !sent[0].subject.contains("New Lead")
    );
    assert!(!sent[0]
        .headers
        .clone()
        .unwrap_or_default()
        .iter()
        .any(|(k, _)| k == "In-Reply-To"));
    assert!(sent[0].html_body.contains("Nary"), "{}", sent[0].html_body);
    let ack = cards(&env, "email.ack.send").await;
    assert!(ack[0].status == "applied" && ack[0].level == 3);
}

#[tokio::test]
async fn test_a_whatsapp_form_waits_for_its_chat_and_sends_nothing() {
    let env = setup("n7_w3f_whatsapp").await;
    let (raw, _) = load("whatsapp_multiline.eml");
    assert!(ingest(&env, &raw, 1, "INBOX").await);
    let msg = last_message(&env).await;
    let mut c = conn(&env).await;
    set(&mut c, "nary.ack.enabled", "true").await;
    dispatch(&env).await;
    let lead = lead_for(&env, msg.id).await;
    assert_eq!(lead.source, "whatsapp");
    let attr = lead.attribution.clone().unwrap();
    assert!(attr["referrer_host"] == "chatgpt.com" && attr["utm_source"] == "chatgpt.com");
    assert_eq!(attr["timezone"], "Asia/Riyadh");
    assert_eq!(
        lead.raw_channel_data.clone().unwrap()["locale"]["country"],
        "SA"
    );
    assert!(env.sent.lock().unwrap().is_empty()); // a [whatsapp] form gets nothing by email
    let arrival: NaryExpectedArrival =
        sqlx::query_as("SELECT * FROM nary_expected_arrivals WHERE lead_id = $1")
            .bind(lead.id)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert!(arrival.status == "pending" && arrival.phone.as_deref() == Some("+966501234567"));
    assert!(((arrival.due_at - arrival.created_at).num_seconds() - 120 * 60).abs() <= 5);
    let (src, refr): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT utm_source, referrer_url FROM lead_attributions WHERE lead_id = $1")
            .bind(lead.id)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert!(
        src.as_deref() == Some("chatgpt.com") && refr.as_deref() == Some("https://chatgpt.com/")
    );
}

#[tokio::test]
async fn test_a_form_without_email_uses_the_timezone_and_phone_and_never_mails() {
    let env = setup("n7_w3f_no_email").await;
    let (raw, _) = load("whatsapp_no_email.eml");
    assert!(ingest(&env, &raw, 1, "INBOX").await);
    let msg = last_message(&env).await;
    dispatch(&env).await;
    let lead = lead_for(&env, msg.id).await;
    assert_eq!(
        lead.raw_channel_data.clone().unwrap()["locale"]["country"],
        "PH"
    ); // the phone code wins
    let person = contact(&env, lead.contact_id.unwrap()).await;
    assert!(person.email.is_none() && person.whatsapp.as_deref() == Some("+639171234567"));
}

#[tokio::test]
async fn test_a_second_submission_merges_into_the_open_lead() {
    let env = setup("n7_w3f_merge").await;
    let (raw, _) = load("web3forms_second_lead.eml");
    assert!(ingest(&env, &raw, 11, "INBOX").await);
    let first = last_message(&env).await;
    dispatch(&env).await;
    let again = String::from_utf8_lossy(&raw).replace("Message-ID: <", "Message-ID: <again-");
    assert!(ingest(&env, again.as_bytes(), 12, "INBOX").await);
    dispatch(&env).await;
    let lead = lead_for(&env, first.id).await;
    let mut c = conn(&env).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM leads WHERE contact_id = $1")
        .bind(lead.contact_id)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(n, 1); // one lead, not two
    assert_eq!(
        lead.raw_channel_data.unwrap()["submissions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn test_spam_forms_are_disqualified_and_never_acknowledged() {
    let env = setup("n7_w3f_spam").await;
    env.brain.set(
        "nary.triage",
        Answer::json(json!({"verdict": "spam", "language": "en", "reason": "SEO offer"})),
    );
    env.brain.set(
        "nary.cascade_review",
        Answer::json(json!({"verdict": "spam", "language": "en", "reason": "still SEO"})),
    );
    let (raw, _) = load("inline_web3forms_support.eml");
    assert!(ingest(&env, &raw, 1, "INBOX").await);
    let msg = last_message(&env).await;
    let mut c = conn(&env).await;
    set(&mut c, "nary.ack.enabled", "true").await;
    dispatch(&env).await;
    let lead = lead_for(&env, msg.id).await;
    assert!(
        lead.status.as_deref() == Some("disqualified")
            && lead.disqualification_reason.as_deref() == Some("spam")
    );
    assert!(env.sent.lock().unwrap().is_empty() && cards(&env, "email.ack.send").await.is_empty());
}

#[tokio::test]
async fn test_missing_fields_use_the_ai_and_warn_the_owner() {
    let env = setup("n7_w3f_drift").await;
    let raw = b"From: Web Form <notify+zz@web3forms.com>\r\nReply-To: sarah@contoso.example\r\n\
DKIM-Signature: v=1; a=rsa-sha256; d=web3forms.com; s=x; b=y\r\nTo: hello@example.com\r\n\
Subject: Something new\r\nMessage-ID: <drift-1@web3forms.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\
Full name -> Sarah Doe\r\nOrganisation -> Contoso Ltd\r\nNote -> need 5 laptops\r\n";
    env.brain.set(
        "nary.web3forms_extract",
        Answer::json(
            json!({"name": "Sarah Doe", "email": null, "phone": null, "company": "Contoso Ltd",
                            "message": "need 5 laptops", "submission_source": "web3forms",
                            "interested_in": null, "from_page": null, "timezone": null,
                            "landing_page": null, "first_referrer": null}),
        ),
    );
    assert!(ingest(&env, raw, 99, "INBOX").await);
    let mut c = conn(&env).await;
    let msg: CrmEmailMessage = sqlx::query_as(
        "SELECT * FROM crm_email_messages WHERE message_id_header = '<drift-1@web3forms.com>'",
    )
    .fetch_one(&mut *c)
    .await
    .unwrap();
    dispatch(&env).await;
    assert!(env
        .brain
        .jobs()
        .contains(&"nary.web3forms_extract".to_string()));
    let lead = lead_for(&env, msg.id).await;
    let person = contact(&env, lead.contact_id.unwrap()).await;
    assert!(
        person.first_name == "Sarah" && person.email.as_deref() == Some("sarah@contoso.example")
    );
    let body: Option<String> = sqlx::query_scalar(
        "SELECT body FROM notifications WHERE type = 'nary_error' AND title LIKE '%form email%' LIMIT 1",
    )
    .fetch_optional(&mut *c)
    .await
    .unwrap()
    .flatten();
    assert!(body.unwrap().contains("AI fallback"));
}

#[tokio::test]
async fn test_junk_only_lets_form_emails_through() {
    let env = setup("n7_w3f_junk").await;
    let (raw, _) = load("web3forms_laptops.eml");
    let other =
        b"From: promo@shop.example\r\nSubject: SALE\r\nMessage-ID: <junk-1@shop>\r\n\r\nbuy now";
    assert!(!ingest(&env, other, 5, "SPAM").await);
    let junk = String::from_utf8_lossy(&raw).replace("Message-ID: <", "Message-ID: <junk-");
    assert!(ingest(&env, junk.as_bytes(), 6, "SPAM").await);
    let mut c = conn(&env).await;
    let froms: Vec<String> =
        sqlx::query_scalar("SELECT from_email FROM crm_email_messages WHERE imap_folder = 'SPAM'")
            .fetch_all(&mut *c)
            .await
            .unwrap();
    assert_eq!(froms, vec!["notify+ao1xd8@web3forms.com".to_string()]);
}

// ─── Email intake (tests/test_nary_email_intake.py) ───────────────────────────

const SUBJECT_TAG: &str = "pytest-email-intake";

fn sender() -> String {
    format!(
        "rachel.{}@vandelay-pytest.example",
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

/// `raw_email(sender, body, name=..., subject=..., headers=..., when=..., reply_to_id=...)`.
fn raw_email(
    sender: &str,
    body: &str,
    name: &str,
    subject: &str,
    headers: &[(&str, &str)],
    when: Option<chrono::DateTime<Utc>>,
    reply_to_id: Option<&str>,
) -> (Vec<u8>, String) {
    let mid = format!("<{}@pytest.local>", Uuid::new_v4().simple());
    let mut s = format!(
        "From: {name} <{sender}>\r\nTo: {MAILBOX}\r\nSubject: {subject} {SUBJECT_TAG}\r\nMessage-ID: {mid}\r\nDate: {}\r\n",
        (when.unwrap_or_else(Utc::now)).format("%a, %d %b %Y %H:%M:%S +0000")
    );
    if let Some(r) = reply_to_id {
        s.push_str(&format!("In-Reply-To: {r}\r\n"));
    }
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("Content-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: 8bit\r\nMIME-Version: 1.0\r\n\r\n");
    s.push_str(&body.replace('\n', "\r\n"));
    s.push_str("\r\n");
    (s.into_bytes(), mid)
}

async fn deliver(env: &Env, raw: &[u8]) -> CrmEmailMessage {
    assert!(
        ingest(
            env,
            raw,
            (Uuid::new_v4().as_u128() % 100_000_000) as i64,
            "INBOX"
        )
        .await
    );
    last_message(env).await
}

async fn email_on(env: &Env) {
    let mut c = conn(env).await;
    set(&mut c, "nary.email.intake_enabled", "true").await;
}

#[tokio::test]
async fn test_a_newsletter_never_reaches_nary() {
    let env = setup("n7_ei_newsletter").await;
    email_on(&env).await;
    let (raw, _) = raw_email(
        &sender(),
        "This month's promo!",
        "Rachel Wilson",
        "Laptop question",
        &[("List-Unsubscribe", "<https://x.test/u>")],
        None,
        None,
    );
    let msg = deliver(&env, &raw).await;
    assert!(msg.auth.unwrap()["nary_skip"]
        .as_str()
        .unwrap()
        .starts_with("mailing list"));
    assert!(events_of(&env, "email.new").await.is_empty());
    dispatch(&env).await;
    assert!(env.brain.calls().is_empty());
}

#[tokio::test]
async fn test_old_mail_found_by_a_sync_is_history() {
    let env = setup("n7_ei_history").await;
    email_on(&env).await;
    let (raw, _) = raw_email(
        &sender(),
        "Hello, please send a quotation",
        "Rachel Wilson",
        "Laptop question",
        &[],
        Some(Utc::now() - Duration::days(30)),
        None,
    );
    deliver(&env, &raw).await;
    assert!(events_of(&env, "email.new").await.is_empty());
}

#[tokio::test]
async fn test_a_new_prospect_becomes_a_lead_is_acknowledged_and_answered() {
    let env = setup("n7_ei_prospect").await;
    email_on(&env).await;
    let mut c = conn(&env).await;
    owner(&mut c).await;
    let addr = sender();
    let (raw, _) = raw_email(
        &addr,
        "Good afternoon, we need laptops for 12 staff at our warehouse.\n\nRina\n+44 20 7946 0958",
        "Rachel Wilson",
        "Laptop question",
        &[],
        None,
        None,
    );
    let msg = deliver(&env, &raw).await;
    assert_eq!(events_of(&env, "email.new").await.len(), 1);
    set(&mut c, "nary.ack.enabled", "true").await;
    let (_, failed) = dispatch(&env).await;
    assert_eq!(failed, 0);
    let (tc, tl) = thread_links(&env, msg.thread_id).await;
    let lead: Lead = sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(tl.unwrap())
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let person = contact(&env, lead.contact_id.unwrap()).await;
    let verdict: NaryTriageVerdict =
        sqlx::query_as("SELECT * FROM nary_triage_verdicts WHERE subject_ref = $1")
            .bind(msg.thread_id.to_string())
            .fetch_one(&mut *c)
            .await
            .unwrap();
    let stored = message(&env, msg.id).await;
    assert!(lead.source == "email" && lead.status.as_deref() == Some("qualifying"));
    assert_eq!(lead.title.as_deref(), Some("Email: Rachel Wilson"));
    assert!(person.email.as_deref() == Some(addr.as_str()) && person.first_name == "Rachel");
    assert_eq!(person.phone.as_deref(), Some("+442079460958"));
    assert_eq!(tc, Some(person.id));
    assert!(verdict.subject_type == "email_thread" && verdict.verdict == "prospect");
    assert!(stored.nary_processed_at.is_some());
    let sent = env.sent.lock().unwrap().clone();
    assert!(
        sent.len() == 1
            && sent[0].to == vec![addr.clone()]
            && sent[0].subject.starts_with("Re: Laptop question")
    );
    let irt = sent[0]
        .headers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .find(|(k, _)| k == "In-Reply-To")
        .map(|x| x.1);
    assert_eq!(irt.as_deref(), Some(stored.message_id_header.as_str()));
    let statuses: Vec<String> = cards(&env, "email.reply.send")
        .await
        .into_iter()
        .map(|c| c.status)
        .collect();
    assert_eq!(statuses, vec!["shadow"]);
    let statuses: Vec<String> = cards(&env, "lead.promote")
        .await
        .into_iter()
        .map(|c| c.status)
        .collect();
    assert_eq!(statuses, vec!["pending"]);
    assert!(env.brain.jobs().contains(&"nary.triage".to_string()));
}

#[tokio::test]
async fn test_a_second_email_in_the_thread_goes_to_the_playbook_not_to_intake() {
    let env = setup("n7_ei_second").await;
    email_on(&env).await;
    let mut c = conn(&env).await;
    owner(&mut c).await;
    let addr = sender();
    let (raw, mid) = raw_email(
        &addr,
        "We need laptops for 3 office desks.",
        "Rachel Wilson",
        "Laptop question",
        &[],
        None,
        None,
    );
    let first = deliver(&env, &raw).await;
    dispatch(&env).await;
    let (raw2, _) = raw_email(
        &addr,
        "Oh, and our budget is around 50 million.",
        "Rachel Wilson",
        "Re: Laptop question",
        &[],
        None,
        Some(&mid),
    );
    let second = deliver(&env, &raw2).await;
    assert_eq!(second.thread_id, first.thread_id);
    assert_eq!(events_of(&env, "email.new").await.len(), 1);
    assert_eq!(events_of(&env, "email.inbound").await.len(), 1);
    dispatch(&env).await;
    assert_eq!(
        env.brain
            .jobs()
            .iter()
            .filter(|j| *j == "nary.triage")
            .count(),
        1
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM leads WHERE source = 'email' AND source_detail = $1",
    )
    .bind(&addr)
    .fetch_one(&mut *c)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn test_spam_is_filtered_and_its_thread_never_read_again() {
    let env = setup("n7_ei_spam").await;
    email_on(&env).await;
    env.brain.set(
        "nary.triage",
        Answer::json(json!({"verdict": "spam", "language": "en", "reason": "SEO offer"})),
    );
    env.brain.set(
        "nary.cascade_review",
        Answer::json(json!({"verdict": "spam", "language": "en", "reason": "SEO offer"})),
    );
    let addr = sender();
    let (raw, mid) = raw_email(
        &addr,
        "We can rank your site #1 on Google, cheap!",
        "Rachel Wilson",
        "SEO",
        &[],
        None,
        None,
    );
    let msg = deliver(&env, &raw).await;
    dispatch(&env).await;
    assert_eq!(thread_links(&env, msg.thread_id).await, (None, None));
    let n = env.brain.calls().len();
    let (raw2, _) = raw_email(
        &addr,
        "Just following up!",
        "Rachel Wilson",
        "Re: SEO",
        &[],
        None,
        Some(&mid),
    );
    deliver(&env, &raw2).await;
    dispatch(&env).await;
    assert!(env.brain.calls().len() == n && env.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_a_phone_in_the_signature_finds_a_known_contact_with_an_open_lead() {
    let env = setup("n7_ei_phone").await;
    email_on(&env).await;
    let mut c = conn(&env).await;
    owner(&mut c).await;
    let contact_id: Uuid = sqlx::query_scalar(
        "INSERT INTO contacts (id, first_name, last_name, phone, email, custom, is_deleted) \
         VALUES ($1, 'Rob', 'Pytest', '+819099998888', 'rob.office@vandelay-pytest.example', '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .fetch_one(&mut *c)
    .await
    .unwrap();
    let lead_id: Uuid = sqlx::query_scalar(
        "INSERT INTO leads (id, contact_id, source, title, status, custom, is_deleted) \
         VALUES ($1, $2, 'whatsapp_direct', 'WhatsApp: Rob', 'qualifying', '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(contact_id)
    .fetch_one(&mut *c)
    .await
    .unwrap();
    let (raw, _) = raw_email(
        "rob.personal@gmail.com",
        "Rob here, following up on yesterday's quotation.\n\nRobert\n+81 90-9999-8888",
        "Rob",
        "Laptop question",
        &[],
        None,
        None,
    );
    let msg = deliver(&env, &raw).await;
    dispatch(&env).await;
    assert_eq!(
        thread_links(&env, msg.thread_id).await,
        (Some(contact_id), Some(lead_id))
    );
    assert!(!env.brain.jobs().contains(&"nary.triage".to_string())); // a known lead: straight to the playbook
    assert_eq!(events_of(&env, "email.inbound").await.len(), 1);
}

async fn old_customer(env: &Env, addr: &str) -> Uuid {
    let mut c = conn(env).await;
    sqlx::query_scalar(
        "INSERT INTO contacts (id, first_name, last_name, email, custom, is_deleted) \
         VALUES ($1, 'Sarah', 'Miller', $2, '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(addr)
    .fetch_one(&mut *c)
    .await
    .unwrap()
}

async fn decide(env: &Env, card: Uuid, me: Uuid, decision: &str, reason: Option<&str>) {
    let mut c = conn(env).await;
    let user = nary::roles::load_actor(&mut c, me).await.unwrap().unwrap();
    nary::executor::decide(&mut c, card, &user, decision, None, reason)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_a_returning_contact_by_email_waits_for_the_owner_then_gets_a_lead() {
    let env = setup("n7_ei_returning").await;
    email_on(&env).await;
    let mut c = conn(&env).await;
    let me = owner(&mut c).await;
    let addr = sender();
    let contact_id = old_customer(&env, &addr).await;
    let (raw, _) = raw_email(
        &addr,
        "Hello, we would like 5 more laptops for our new branch.",
        "Sarah Miller",
        "Laptop question",
        &[],
        None,
        None,
    );
    let msg = deliver(&env, &raw).await;
    set(&mut c, "nary.ack.enabled", "true").await;
    dispatch(&env).await;
    assert_eq!(
        thread_links(&env, msg.thread_id).await,
        (Some(contact_id), None)
    );
    let card = cards(&env, "lead.open_returning").await.remove(0);
    assert!(
        card.status == "pending" && card.payload["thread_id"] == json!(msg.thread_id.to_string())
    );
    assert!(
        env.sent.lock().unwrap().is_empty() && cards(&env, "email.reply.send").await.is_empty()
    );
    decide(&env, card.id, me, "apply", None).await;
    dispatch(&env).await;
    let (_, tl) = thread_links(&env, msg.thread_id).await;
    let lead: Lead = sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(tl.unwrap())
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert!(lead.contact_id == Some(contact_id) && lead.source == "email");
    assert_eq!(cards(&env, "email.reply.send").await.len(), 1);
}

#[tokio::test]
async fn test_a_rejected_card_keeps_nary_out_of_that_conversation() {
    let env = setup("n7_ei_rejected").await;
    email_on(&env).await;
    let mut c = conn(&env).await;
    let me = owner(&mut c).await;
    let addr = sender();
    old_customer(&env, &addr).await;
    let (raw, mid) = raw_email(
        &addr,
        "Mr, we have not received last month's invoice.",
        "Sarah Miller",
        "Laptop question",
        &[],
        None,
        None,
    );
    deliver(&env, &raw).await;
    dispatch(&env).await;
    let card = cards(&env, "lead.open_returning").await.remove(0);
    decide(&env, card.id, me, "reject", Some("I'll handle this one")).await;
    let n = env.brain.calls().len();
    let (raw2, _) = raw_email(
        &addr,
        "Please check, Mr.",
        "Sarah Miller",
        "Re: Laptop question",
        &[],
        None,
        Some(&mid),
    );
    deliver(&env, &raw2).await;
    dispatch(&env).await;
    assert!(env.brain.calls().len() == n && cards(&env, "lead.open_returning").await.len() == 1);
    assert!(env.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_a_owner_correction_to_business_reads_the_email_again() {
    // The router (POST /nary/intake/verdicts/{id}/correct) is wave 4: here its two service calls,
    // triage.correct then email_intake.requeue, run directly.
    let env = setup("n7_ei_correction").await;
    email_on(&env).await;
    let mut c = conn(&env).await;
    let me = owner(&mut c).await;
    env.brain.set(
        "nary.triage",
        Answer::json(json!({"verdict": "vendor", "language": "en", "reason": "sells something"})),
    );
    env.brain.set(
        "nary.cascade_review",
        Answer::json(json!({"verdict": "vendor", "language": "en", "reason": "sells something"})),
    );
    let (raw, _) = raw_email(
        &sender(),
        "We are a laptop distributor, interested in a partnership and in buying laptops for our clients.",
        "Rachel Wilson",
        "Laptop question",
        &[],
        None,
        None,
    );
    let msg = deliver(&env, &raw).await;
    dispatch(&env).await;
    let verdict: NaryTriageVerdict =
        sqlx::query_as("SELECT * FROM nary_triage_verdicts WHERE subject_ref = $1")
            .bind(msg.thread_id.to_string())
            .fetch_one(&mut *c)
            .await
            .unwrap();
    env.brain.set(
        "nary.triage",
        Answer::json(json!({"verdict": "prospect", "language": "en", "reason": "wants to buy"})),
    );
    let user = nary::roles::load_actor(&mut c, me).await.unwrap().unwrap();
    let mut tx = c.begin().await.unwrap();
    let row = triage::correct(&mut tx, verdict.id, "prospect", &user, None)
        .await
        .unwrap();
    assert!(email_intake::requeue(&mut tx, &row.subject_ref, "prospect")
        .await
        .unwrap());
    tx.commit().await.unwrap();
    dispatch(&env).await;
    assert!(thread_links(&env, msg.thread_id).await.1.is_some());
}

/// `wa`: a fake WhatsApp sidecar (one per process): /send answers with a message id; everything else 404.
async fn wa_server() -> &'static wiremock::MockServer {
    static WA: tokio::sync::OnceCell<wiremock::MockServer> = tokio::sync::OnceCell::const_new();
    WA.get_or_init(|| async {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/send"))
            .respond_with(|req: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let to = body["to"].as_str().unwrap_or("").to_string();
                wiremock::ResponseTemplate::new(200).set_body_json(json!({
                    "success": true, "chatId": to, "messageId": format!("true_{to}_{}", Uuid::new_v4().simple())}))
            })
            .mount(&server)
            .await;
        std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
        std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
        server
    })
    .await
}

async fn wa_sent(server: &wiremock::MockServer) -> usize {
    let reqs = server.received_requests().await.unwrap_or_default();
    reqs.iter().filter(|r| r.url.path() == "/send").count()
}

#[tokio::test]
async fn test_a_returning_contact_on_whatsapp_waits_for_the_owner() {
    let env = setup("n7_ei_returning_wa").await;
    let wa = wa_server().await;
    let before = wa_sent(wa).await;
    let mut c = conn(&env).await;
    let me = owner(&mut c).await;
    let number = format!(
        "8190{:08}",
        Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000
    );
    let cid = format!("{number}@c.us");
    let contact_id: Uuid = sqlx::query_scalar(
        "INSERT INTO contacts (id, first_name, last_name, whatsapp, phone, custom, is_deleted) \
         VALUES ($1, 'Sarah', 'Miller', $2, $2, '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(format!("+{number}"))
    .fetch_one(&mut *c)
    .await
    .unwrap();
    let name = format!("Bob Pytest{}", &Uuid::new_v4().simple().to_string()[..6]);
    let p = json!({"messageId": format!("false_{cid}_{}", &Uuid::new_v4().simple().to_string()[..20].to_uppercase()),
        "chatId": cid, "from": cid, "to": "818000@c.us",
        "body": "Hello Mr, this is Sarah. We would like to order more laptops for our new office.", "type": "chat",
        "timestamp": Utc::now().timestamp(), "hasMedia": false, "fromMe": false, "ack": 1, "chatName": name,
        "isGroup": false, "fromName": name, "isMyContact": false});
    let mut tx = c.begin().await.unwrap();
    nary::wa_ingest::ingest(&mut tx, &p, "live").await.unwrap();
    tx.commit().await.unwrap();
    set(&mut c, "nary.ack.enabled", "true").await;
    dispatch(&env).await;
    let chat: granary_db::rows::nary::WaChat =
        sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
            .bind(&cid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert!(chat.contact_id == Some(contact_id) && chat.lead_id.is_none());
    let card = cards(&env, "lead.open_returning").await.remove(0);
    assert!(card.status == "pending" && card.payload["chat_id"] == json!(cid));
    assert!(wa_sent(wa).await == before && cards(&env, "whatsapp.reply.send").await.is_empty()); // no ack, no reply yet
    decide(&env, card.id, me, "apply", None).await;
    dispatch(&env).await;
    // Fix batch 1: the reply is a reply job that waits nary.reply.debounce_seconds for more messages.
    sqlx::query(
        "UPDATE nary_reply_jobs SET due_at = now() - interval '1 second' WHERE status = 'pending'",
    )
    .execute(&mut *c)
    .await
    .unwrap();
    nary::reply_guard::process_due(&mut c).await.unwrap();
    set(&mut c, "nary.ack.enabled", "false").await;
    let chat: granary_db::rows::nary::WaChat =
        sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
            .bind(&cid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    let lead: Lead = sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(chat.lead_id.unwrap())
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert!(lead.contact_id == Some(contact_id) && lead.source == "whatsapp_direct");
    assert_eq!(cards(&env, "whatsapp.reply.send").await.len(), 1);
    assert_eq!(cards(&env, "lead.open_returning").await.len(), 1);
}

#[tokio::test]
#[ignore = "covered by compat-tests/http-suite/tests/test_nary_rows.py::test_the_filtered_list_and_correction_api (wave 4)"]
async fn test_the_filtered_list_and_correction_api() {}
