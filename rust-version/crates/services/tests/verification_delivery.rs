//! Port of backend/tests/test_document_delivery.py (the send-to-client tests), plus
//! test_nary_p4_documents.py `test_sending_a_proposal_attaches_its_quotations` and test_nary_chat_fixes.py
//! `test_an_email_card_carries_an_approved_quotation_as_its_pdf`, at the service level: the HTTP wrappers
//! (`/documents/{type}/{id}/send`, `/prepare-send`) are routers-wave, so these call `delivery` the way
//! those handlers do. Python's fakes map to: `delivery::seams` (Redis, `_send_email`, `_verified_pdf`,
//! `_emit_quotation`), `email::set_global_seams` (`load_crm_smtp`, `_send_via_config`), and one fake HTTP
//! server for the PDF service, MinIO and the WhatsApp service.

mod verification_common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_services::approvals::adapters::{get_adapter, Entity};
use granary_services::approvals::engine;
use granary_services::email::{Capture, Seams, SendCall, SmtpConfig};
use granary_services::verification::delivery::{self, seams, GuardRedis};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use verification_common as c;

const TAG: &str = "verif_delivery";
const SEND_PERMS: &[&str] = &["approvals:view", "sales:send_quotation"];

static EMAILS: Mutex<Vec<SendCall>> = Mutex::new(Vec::new());
static EVENTS: Mutex<Vec<(String, Uuid, Value)>> = Mutex::new(Vec::new());
static SENT_FOLDER: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// Just enough of redis.asyncio for the resend guard (Python's FakeRedis).
#[derive(Default)]
struct FakeRedis {
    store: Mutex<HashMap<String, String>>,
}

#[async_trait::async_trait]
impl GuardRedis for FakeRedis {
    async fn set(&self, key: &str, value: &str, nx: bool, _ex: u64) -> anyhow::Result<bool> {
        let mut s = self.store.lock().unwrap();
        if nx && s.contains_key(key) {
            return Ok(false);
        }
        s.insert(key.to_string(), value.to_string());
        Ok(true)
    }
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self.store.lock().unwrap().get(key).cloned())
    }
    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.store.lock().unwrap().remove(key);
        Ok(())
    }
}

/// The autouse fixtures: fake Redis, the outbox (email + WhatsApp captured), events recorded.
async fn setup() -> (
    tokio::sync::MutexGuard<'static, ()>,
    Arc<FakeRedis>,
    PgConnection,
) {
    let guard = c::serial().await;
    granary_services::approvals::notify::set_external_delivery_enabled(false);
    seams::reset();
    EMAILS.lock().unwrap().clear();
    EVENTS.lock().unwrap().clear();
    SENT_FOLDER.lock().unwrap().clear();
    seams::set_save_to_sent(Some(Arc::new(|raw| {
        Box::pin(async move {
            SENT_FOLDER.lock().unwrap().push(raw);
        })
    })));
    let r = Arc::new(FakeRedis::default());
    let r2 = r.clone();
    seams::set_redis(Some(Arc::new(move || {
        let r = r2.clone();
        Box::pin(async move { Ok(r as Arc<dyn GuardRedis>) })
    })));
    seams::set_emit_quotation(Some(Arc::new(|_db, type_, q, _user, extra| {
        Box::pin(async move {
            EVENTS
                .lock()
                .unwrap()
                .push((type_, q.id, Value::Object(extra)));
            Ok(())
        })
    })));
    granary_services::email::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(|call: SendCall| {
            Box::pin(async move {
                let wants = call.wants_capture;
                let mid = call.headers.as_ref().and_then(|h| {
                    h.iter()
                        .find(|(k, _)| k == "Message-ID")
                        .map(|(_, v)| v.clone())
                });
                EMAILS.lock().unwrap().push(call);
                let cap = wants.then(|| Capture {
                    raw: Some(b"raw message bytes".to_vec()),
                    message_id: mid,
                });
                Ok((true, cap))
            })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: String::new(),
                    password: String::new(),
                    from_name: "ACME".into(),
                    from_email: "sales@example.com".into(),
                    tls: true,
                })
            })
        })),
    }));
    let db = c::session(TAG).await;
    (guard, r, db)
}

fn detail(e: &ApiError) -> Value {
    match e {
        ApiError::Http { detail, .. } => detail.clone(),
        other => panic!("not an HTTPException: {other:?}"),
    }
}

async fn load(db: &mut PgConnection, entity_type: &str, id: Uuid) -> Entity {
    get_adapter(entity_type)
        .unwrap()
        .load(db, &id.to_string())
        .await
        .unwrap()
        .unwrap()
}

/// `_quote_for_contact`: a quotation for a CRM contact John Scott (WhatsApp +81 90 1234 5678).
async fn quote_for_contact(db: &mut PgConnection, catalog: bool) -> (Uuid, String) {
    let product = if catalog {
        Some(c::make_product(db).await)
    } else {
        None
    };
    let qid = c::make_quotation(db, product).await;
    let (cid, email) =
        c::make_contact(db, "John", Some("Scott"), Some("+81 90 1234 5678"), None).await;
    sqlx::query("UPDATE quotations SET customer_contact_id = $2 WHERE id = $1")
        .bind(qid)
        .bind(cid)
        .execute(&mut *db)
        .await
        .unwrap();
    c::commit(db).await;
    (qid, email)
}

#[allow(clippy::too_many_arguments)]
async fn send(
    db: &mut PgConnection,
    entity_type: &str,
    id: Uuid,
    user: &AuthUser,
    channel: &str,
    to: &[&str],
    cc: &[&str],
    message: Option<&str>,
    force: bool,
) -> Result<Value, ApiError> {
    let mut e = load(db, entity_type, id).await;
    let to: Vec<String> = to.iter().map(|s| s.to_string()).collect();
    let cc: Vec<String> = cc.iter().map(|s| s.to_string()).collect();
    let out = delivery::send(
        db,
        entity_type,
        &mut e,
        user,
        channel,
        &to,
        Some(&cc),
        message,
        None,
        force,
    )
    .await;
    if out.is_ok() {
        c::commit(db).await;
    } else {
        sqlx::query("ROLLBACK").execute(&mut *db).await.unwrap();
        sqlx::query("BEGIN").execute(&mut *db).await.unwrap();
    }
    out
}

async fn status_of(db: &mut PgConnection, table: &str, id: Uuid) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT status FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

// --- Send -------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_nothing_is_sent_before_approval() {
    let (_g, _r, mut db) = setup().await;
    let bdr = c::make_person(&mut db, &["sales_bizdev"], SEND_PERMS, false).await;
    let (qid, email) = quote_for_contact(&mut db, false).await; // custom line: the Owner must approve
    let r = send(
        &mut db,
        "quotation",
        qid,
        &bdr,
        "email",
        &[&email],
        &[],
        None,
        false,
    )
    .await;
    let e = r.unwrap_err();
    assert_eq!(e.status().as_u16(), 409);
    assert_eq!(detail(&e)["code"], "approval_pending");
    assert!(EMAILS.lock().unwrap().is_empty() && c::WA_SENT.lock().unwrap().is_empty());
    assert_eq!(
        status_of(&mut db, "quotations", qid).await.as_deref(),
        Some("draft")
    );
}

#[tokio::test]
async fn test_prepare_send_submits_then_returns_the_prefilled_recipient() {
    let (_g, _r, mut db) = setup().await;
    // prepare-send = the gate (ensure_or_submit "sent"), then recipient_defaults.
    let bdr = c::make_person(&mut db, &["sales_bizdev"], SEND_PERMS, false).await;
    let (qid, _) = quote_for_contact(&mut db, false).await;
    let e = load(&mut db, "quotation", qid).await;
    let waiting = engine::ensure_or_submit(&mut db, "quotation", &e, &bdr, "sent", None)
        .await
        .unwrap_err();
    assert_eq!(waiting.status().as_u16(), 409);
    assert_eq!(detail(&waiting)["code"], "approval_pending");

    // The Owner sending their own custom quotation approves it on the spot.
    let owner = c::owner(&mut db).await;
    let (own, own_email) = quote_for_contact(&mut db, false).await;
    let e = load(&mut db, "quotation", own).await;
    engine::ensure_or_submit(&mut db, "quotation", &e, &owner, "sent", None)
        .await
        .unwrap();
    let ready = delivery::recipient_defaults(&mut db, "quotation", &e)
        .await
        .unwrap();
    assert_eq!(ready["email"], json!(own_email));
    assert_eq!(ready["whatsapp"], "+81 90 1234 5678");
    assert_eq!(ready["name"], "John Scott");
}

#[tokio::test]
async fn test_approved_quotation_is_emailed_with_its_verified_pdf() {
    let (_g, _r, mut db) = setup().await;
    let bdr = c::make_person(&mut db, &["sales_bizdev"], SEND_PERMS, false).await;
    let (qid, email) = quote_for_contact(&mut db, true).await; // catalog at list price: auto
    let r = send(
        &mut db,
        "quotation",
        qid,
        &bdr,
        "email",
        &[&email],
        &["boss@customer.test"],
        Some("Hi John"),
        false,
    )
    .await
    .unwrap();
    let verify_url = r["verify_url"].as_str().unwrap().to_string();
    assert!(verify_url.rsplit_once('/').unwrap().0.ends_with("/v"));

    let mails = EMAILS.lock().unwrap().clone();
    assert_eq!(mails.len(), 1);
    let mail = &mails[0];
    assert_eq!(mail.cfg.from_email, "sales@example.com");
    assert_eq!(mail.to, vec![email.clone()]);
    assert_eq!(mail.cc, Some(vec!["boss@customer.test".to_string()]));
    let atts = mail.attachments.clone().unwrap();
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0].mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(atts[0].content, b"%PDF-1.4 verified copy");
    assert!(mail.html_body.contains(&verify_url) && mail.html_body.contains("Hi John"));
    assert_eq!(
        status_of(&mut db, "quotations", qid).await.as_deref(),
        Some("sent")
    );
    let (atype, dir): (String, Option<String>) = sqlx::query_as(
        "SELECT activity_type, direction FROM activities WHERE channel_data->>'document_id' = $1",
    )
    .bind(qid.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (atype.as_str(), dir.as_deref()),
        ("email", Some("outbound"))
    );
    // The first send starts the follow-up cadence.
    let events = EVENTS.lock().unwrap().clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "quotation.sent");
    assert_eq!(events[0].2["channel"], "email");
}

#[tokio::test]
async fn test_approved_quotation_goes_by_whatsapp_as_a_pdf() {
    let (_g, _r, mut db) = setup().await;
    let bdr = c::make_person(&mut db, &["sales_bizdev"], SEND_PERMS, false).await;
    let (qid, _) = quote_for_contact(&mut db, true).await;
    let r = send(
        &mut db,
        "quotation",
        qid,
        &bdr,
        "whatsapp",
        &["+81 90 1234 5678"],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    let sent = c::WA_SENT.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let msg = &sent[0];
    assert!(msg["url"].as_str().unwrap().ends_with("/send-media"));
    assert_eq!(msg["to"], "819012345678");
    assert_eq!(msg["mimetype"], "application/pdf");
    assert!(msg["filename"].as_str().unwrap().ends_with(".pdf"));
    assert!(msg["caption"]
        .as_str()
        .unwrap()
        .contains(r["verify_url"].as_str().unwrap()));
}

// test_sending_needs_the_send_permission: the permission is the router's dependency (routers wave).
// test_quotation_pdf_carries_the_approval_stamp_instead_of_a_signature_field, link expiry and catalog
// access test verification.render / service / the catalog router, not delivery.

// --- Invoices, recorded email, resend guard, PDF failure ---------------------------------------------

/// `_invoice(client, **extra)`: an invoice for a CRM contact (sent by the Owner, who approves on the spot).
async fn invoice(
    db: &mut PgConnection,
    email: Option<&str>,
    tel: Option<&str>,
    with_crm: bool,
) -> (Uuid, Option<String>) {
    let (org, contact, contact_email) = if with_crm {
        let org = c::make_org(
            db,
            &c::u("Invoice Corp"),
            Some("billing@org.test"),
            Some("021 555 1234"),
        )
        .await;
        let (cid, cemail) =
            c::make_contact(db, "Sarah", None, Some("+81 80 1111 2222"), Some(org)).await;
        (Some(org), Some(cid), Some(cemail))
    } else {
        (None, None, None)
    };
    let id = Uuid::new_v4();
    let today = chrono::Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO invoices (id, number, invoice_type, revision, status, currency, exchange_rate, subtotal, tax_total, \
         total, amount_paid, amount_due, refund_due, issue_date, due_date, customer_org_id, customer_contact_id, \
         customer_name, customer_email, customer_tel, tax_lines, is_deleted) \
         VALUES ($1, $2, 'commercial', 1, 'draft', 'JPY', 1, 2500000, 0, 2500000, 0, 2500000, 0, $3, '2026-12-31', $4, $5, \
         $6, $7, $8, '[]', false)",
    )
    .bind(id)
    .bind(c::u("PYT-INV"))
    .bind(today)
    .bind(org)
    .bind(contact)
    .bind(if with_crm { None } else { Some("Walk-in Co") })
    .bind(email)
    .bind(tel)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO invoice_items (id, invoice_id, name, quantity, unit_price, amount, discount_pct, discount_amount, \
         tax_pct, tax_amount, sort_order, unit, currency) VALUES ($1, $2, 'Milestone 1', 1, 2500000, 2500000, 0, 0, 0, 0, 0, 'Unit', 'JPY')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
    c::commit(db).await;
    (id, contact_email)
}

#[tokio::test]
async fn test_an_invoice_is_emailed_and_the_email_is_kept_in_the_crm_inbox() {
    let (_g, _r, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let (iid, email) = invoice(&mut db, None, None, true).await;
    let email = email.unwrap();
    send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &[&email],
        &[],
        Some("Invoice attached"),
        false,
    )
    .await
    .unwrap();

    let mails = EMAILS.lock().unwrap().clone();
    assert_eq!(mails.len(), 1);
    let message_id = mails[0]
        .headers
        .clone()
        .unwrap()
        .into_iter()
        .find(|(k, _)| k == "Message-ID")
        .unwrap()
        .1;
    assert!(mails[0].subject.starts_with("Invoice "));
    assert_eq!(
        mails[0].attachments.clone().unwrap()[0]
            .mime_type
            .as_deref(),
        Some("application/pdf")
    );
    assert_eq!(
        *SENT_FOLDER.lock().unwrap(),
        vec![b"raw message bytes".to_vec()]
    ); // a copy went to the Sent folder

    let (msg_id, direction, to_emails, thread_id): (Uuid, Option<String>, Value, Uuid) = sqlx::query_as(
        "SELECT id, direction, to_jsonb(to_emails), thread_id FROM crm_email_messages WHERE message_id_header = $1",
    )
    .bind(&message_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(direction.as_deref(), Some("outbound"));
    assert_eq!(to_emails, json!([email]));
    let (linked, count): (Option<Uuid>, Option<i32>) = sqlx::query_as(
        "SELECT linked_contact_id, message_count FROM crm_email_threads WHERE id = $1",
    )
    .bind(thread_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let contact_id: Uuid =
        sqlx::query_scalar("SELECT customer_contact_id FROM invoices WHERE id = $1")
            .bind(iid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(linked, Some(contact_id));
    assert_eq!(count, Some(1));
    let types: Vec<Option<String>> =
        sqlx::query_scalar("SELECT content_type FROM crm_email_attachments WHERE message_id = $1")
            .bind(msg_id)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert_eq!(types, vec![Some("application/pdf".to_string())]);
    assert_eq!(
        status_of(&mut db, "invoices", iid).await.as_deref(),
        Some("sent")
    );
    let ch: Option<String> = sqlx::query_scalar(
        "SELECT channel_message_id FROM activities WHERE channel_data->>'document_id' = $1",
    )
    .bind(iid.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(ch.as_deref(), Some(message_id.as_str()));
}

#[tokio::test]
async fn test_an_invoice_goes_by_whatsapp() {
    let (_g, _r, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let (iid, _) = invoice(&mut db, None, None, true).await;
    send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "whatsapp",
        &["+81 80 1111 2222"],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    let sent = c::WA_SENT.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["to"], "818011112222");
    assert_eq!(sent[0]["mimetype"], "application/pdf");
}

#[tokio::test]
async fn test_billing_email_and_whatsapp_are_saved_and_used_first() {
    let (_g, _r, mut db) = setup().await;
    let (iid, _) = invoice(
        &mut db,
        Some("finance@cust.test"),
        Some("+81 90 9999 0000"),
        true,
    )
    .await;
    let e = load(&mut db, "invoice", iid).await;
    let prep = delivery::recipient_defaults(&mut db, "invoice", &e)
        .await
        .unwrap();
    assert_eq!(prep["email"], "finance@cust.test");
    assert_eq!(prep["whatsapp"], "+81 90 9999 0000");
}

#[tokio::test]
async fn test_an_invoice_for_a_customer_outside_the_crm_prefills_its_billing_details() {
    let (_g, _r, mut db) = setup().await;
    let (iid, _) = invoice(
        &mut db,
        Some("ap@walkin.test"),
        Some("+81 90 7777 0000"),
        false,
    )
    .await;
    let e = load(&mut db, "invoice", iid).await;
    let prep = delivery::recipient_defaults(&mut db, "invoice", &e)
        .await
        .unwrap();
    assert_eq!(
        prep,
        json!({"name": "Walk-in Co", "email": "ap@walkin.test", "whatsapp": "+81 90 7777 0000"})
    );
}

#[tokio::test]
async fn test_a_second_send_to_the_same_recipient_needs_confirmation() {
    let (_g, _r, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let (iid, email) = invoice(&mut db, None, None, true).await;
    let email = email.unwrap();
    send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &[&email],
        &[],
        None,
        false,
    )
    .await
    .unwrap();

    let again = send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &[&email],
        &[],
        None,
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(again.status().as_u16(), 409);
    assert_eq!(detail(&again)["code"], "recently_sent");
    assert!(detail(&again)["sent_at"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(EMAILS.lock().unwrap().len(), 1); // nothing went out the second time

    // another recipient is fine
    send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &["someone.else@cust.test"],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &[&email],
        &[],
        None,
        true,
    )
    .await
    .unwrap();
    assert_eq!(EMAILS.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn test_a_send_already_in_progress_is_not_started_twice() {
    let (_g, r, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let (iid, email) = invoice(&mut db, None, None, true).await;
    let email = email.unwrap();
    let e = load(&mut db, "invoice", iid).await;
    let key = delivery::_send_key("invoice", &e, "email", std::slice::from_ref(&email));
    r.store
        .lock()
        .unwrap()
        .insert(format!("{key}:inflight"), "1".into()); // the first send is still running

    let err = send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &[&email],
        &[],
        None,
        true,
    )
    .await
    .unwrap_err();
    assert_eq!(err.status().as_u16(), 409);
    assert_eq!(detail(&err)["code"], "send_in_progress");
    assert!(EMAILS.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_sending_still_works_when_redis_is_down() {
    let (_g, _r, mut db) = setup().await;
    seams::set_redis(Some(Arc::new(|| {
        Box::pin(async { Err(anyhow::anyhow!("redis unreachable")) })
    })));
    let owner = c::owner(&mut db).await;
    let (iid, email) = invoice(&mut db, None, None, true).await;
    send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &[&email.unwrap()],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(EMAILS.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_a_pdf_render_failure_is_explained_and_nothing_is_sent() {
    let (_g, r, mut db) = setup().await;
    c::PDF_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
    let owner = c::owner(&mut db).await;
    let (iid, email) = invoice(&mut db, None, None, true).await;
    let err = send(
        &mut db,
        "invoice",
        iid,
        &owner,
        "email",
        &[&email.unwrap()],
        &[],
        None,
        false,
    )
    .await
    .unwrap_err();
    // 10 Nov 2026: 503 (Cloudflare replaces an origin 502 with its own page), with the reason
    assert_eq!(err.status().as_u16(), 503);
    assert!(detail(&err)
        .as_str()
        .unwrap()
        .contains("could not be rendered"));
    assert!(EMAILS.lock().unwrap().is_empty() && c::WA_SENT.lock().unwrap().is_empty());
    assert!(!r
        .store
        .lock()
        .unwrap()
        .keys()
        .any(|k| k.ends_with(":inflight"))); // the lock was released
}

// --- test_nary_p4_documents.py / test_nary_chat_fixes.py ------------------------------------------------

#[tokio::test]
async fn test_sending_a_proposal_attaches_its_quotations() {
    let (_g, _r, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let files: Arc<Mutex<Vec<(Vec<String>, Vec<String>)>>> = Arc::default();
    let f2 = files.clone();
    seams::set_send_email(Some(Arc::new(move |_db, args: delivery::SendEmailArgs| {
        let f = f2.clone();
        Box::pin(async move {
            let mut names = vec![args.filename.clone()];
            names.extend(args.extra.unwrap_or_default().into_iter().map(|(n, _)| n));
            f.lock().unwrap().push((args.to, names));
            Ok("<msg@test>".to_string())
        })
    })));
    let (qid, email) = quote_for_contact(&mut db, true).await;
    let contact_id: Uuid =
        sqlx::query_scalar("SELECT customer_contact_id FROM quotations WHERE id = $1")
            .bind(qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    // 1 Nov 2026: a proposal needs its required figures before approval; this one has none (not the subject here)
    sqlx::query(
        "UPDATE system_settings SET value = '[]' WHERE key = 'proposal.visuals.default_figures'",
    )
    .execute(&mut db)
    .await
    .unwrap();
    let doc_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sales_documents (id, doc_type, number, revision, title, language, contact_id, status, validity_days, \
         cover, is_deleted) VALUES ($1, 'solution_proposal', $2, 1, 'Pytest proposal', 'en', $3, 'draft', 30, '{}', false)",
    )
    .bind(doc_id)
    .bind(c::u("PYT-SP"))
    .bind(contact_id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO sales_document_quotations (id, document_id, quotation_id, position) VALUES ($1, $2, $3, 0)")
        .bind(Uuid::new_v4())
        .bind(doc_id)
        .bind(qid)
        .execute(&mut db)
        .await
        .unwrap();
    c::commit(&mut db).await;
    for (etype, eid) in [("quotation", qid), ("sales_document", doc_id)] {
        let req = engine::submit(&mut db, etype, &eid.to_string(), &owner, None, None, None)
            .await
            .unwrap();
        c::commit(&mut db).await;
        if req.status != "approved" {
            engine::act(
                &mut db,
                &req.id.to_string(),
                &owner,
                "approved",
                None,
                None,
                None,
            )
            .await
            .unwrap();
            c::commit(&mut db).await;
        }
    }
    send(
        &mut db,
        "sales_document",
        doc_id,
        &owner,
        "email",
        &[&email],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    let files = files.lock().unwrap().clone();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].1.len(), 2); // the proposal and its quotation, one email
    assert_eq!(
        status_of(&mut db, "sales_documents", doc_id)
            .await
            .as_deref(),
        Some("sent")
    );
    assert_eq!(
        status_of(&mut db, "quotations", qid).await.as_deref(),
        Some("sent")
    );
}

#[tokio::test]
async fn test_an_email_card_carries_an_approved_quotation_as_its_pdf() {
    // The card/staff-chat half of the Python test is Nary (wave 3); this is the delivery half:
    // email_attachments turns the card's `attach` into the verified PDF.
    let (_g, _r, mut db) = setup().await;
    seams::set_verified_pdf(Some(Arc::new(|_db, _t, _e, _u| {
        Box::pin(async { Ok((None, b"%PDF-1.4 pytest".to_vec())) })
    })));
    let owner = c::owner(&mut db).await;
    let qid = c::make_quotation(&mut db, None).await;
    c::commit(&mut db).await;
    let item = json!({"type": "quotation", "id": qid.to_string()});
    let label = delivery::attachment_label(&mut db, &item)
        .await
        .unwrap()
        .unwrap();
    assert!(label.starts_with("Quotation "));
    let files = delivery::email_attachments(&mut db, &[item], &owner)
        .await
        .unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].filename.ends_with(".pdf"));
    assert_eq!(files[0].content, b"%PDF-1.4 pytest");

    let bad = delivery::attachment_label(&mut db, &json!({"type": "contract", "id": "x"}))
        .await
        .unwrap();
    assert_eq!(
        bad,
        Err("Only quotation, invoice, sales_document can be attached".to_string())
    );
    let gone = delivery::attachment_label(&mut db, &json!({"type": "quotation", "id": "x"}))
        .await
        .unwrap();
    assert_eq!(gone, Err("No quotation with id x".to_string()));
}
