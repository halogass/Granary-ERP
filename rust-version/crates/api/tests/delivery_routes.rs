//! Port of the route-level tests of backend/tests/test_document_delivery.py through routers/delivery.py.
//! Python's fakes map to: the email seams (`load_crm_smtp`, `_send_via_config`), `delivery::seams`
//! (`save_to_sent`), and one wiremock server for the PDF service, MinIO and the WhatsApp service
//! (verification_common). The resend guard runs on the test Redis (the app's real `_redis`), which is what
//! Python's FakeRedis stands in for.
//!
//! Invoices are inserted as rows (the /finance/invoices route is group 8's): the resend guard and the walk-in
//! billing prefill run on them, as in Python. The remaining invoice variants (CRM inbox copy, WhatsApp,
//! billing-first, Redis down, PDF failure on an invoice) exercise the same delivery code as the services tests
//! (crates/services/tests/verification_delivery.rs) and the quotation tests here.

mod support;
mod verification_common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use granary_services::email::{Capture, Seams, SendCall, SmtpConfig};
use granary_services::verification::delivery::seams;
use serde_json::{json, Value};
use uuid::Uuid;
use verification_common as c;

const SEND_PERMS: &[&str] = &["approvals:view", "sales:send_quotation"];

static EMAILS: Mutex<Vec<SendCall>> = Mutex::new(Vec::new());
static SENT_FOLDER: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// The autouse fixtures: the outbox (email captured, a copy to the Sent folder), one test at a time.
async fn setup() -> (
    &'static support::TestApp,
    tokio::sync::MutexGuard<'static, ()>,
) {
    let app = c::app("delivery");
    let g = app.serial().await;
    app.ensure_crm_mailbox("sales@example.test").await;
    EMAILS.lock().unwrap().clear();
    SENT_FOLDER.lock().unwrap().clear();
    c::WA_SENT.lock().unwrap().clear();
    c::PDF_DOWN.store(false, Ordering::SeqCst);
    seams::set_save_to_sent(Some(Arc::new(|raw| {
        Box::pin(async move {
            SENT_FOLDER.lock().unwrap().push(raw);
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
                    from_name: "Your Company".into(),
                    from_email: "sales@example.test".into(),
                    tls: true,
                })
            })
        })),
    }));
    (app, g)
}

async fn quote(app: &support::TestApp, catalog: bool) -> (Uuid, String) {
    let mut db = app.db().await;
    let product = if catalog {
        Some(c::make_product(&mut db).await)
    } else {
        None
    };
    c::make_quotation(&mut db, product, true).await
}

async fn send(app: &support::TestApp, token: &str, qid: Uuid, body: Value) -> support::Resp {
    app.post(
        token,
        &format!("/api/v1/documents/quotation/{qid}/send"),
        body,
    )
    .await
}

#[tokio::test]
async fn test_nothing_is_sent_before_approval() {
    let (app, _g) = setup().await;
    let bdr = c::make_person(app, &["sales_bizdev"], SEND_PERMS).await;
    let (qid, email) = quote(app, false).await; // custom line: the Owner must approve
    let r = send(
        app,
        &bdr.token,
        qid,
        json!({"channel": "email", "to": [email]}),
    )
    .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.json()["detail"]["code"], "approval_pending");
    assert!(EMAILS.lock().unwrap().is_empty() && c::WA_SENT.lock().unwrap().is_empty());
    let mut db = app.db().await;
    assert_eq!(
        c::status_of(&mut db, "quotations", qid).await.as_deref(),
        Some("draft")
    );
    // the submission was committed although the request failed
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approval_requests WHERE entity_id = $1")
        .bind(qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn test_prepare_send_submits_then_returns_the_prefilled_recipient() {
    let (app, _g) = setup().await;
    let bdr = c::make_person(app, &["sales_bizdev"], SEND_PERMS).await;
    let (qid, _) = quote(app, false).await;
    let waiting = app
        .post(
            &bdr.token,
            &format!("/api/v1/documents/quotation/{qid}/prepare-send"),
            json!({}),
        )
        .await;
    assert_eq!(waiting.status, 409);
    assert_eq!(waiting.json()["detail"]["code"], "approval_pending");

    // The Owner sending their own custom quotation approves it on the spot.
    let sa = c::owner(app).await;
    let (own, own_email) = quote(app, false).await;
    let ready = app
        .post(
            &sa.token,
            &format!("/api/v1/documents/quotation/{own}/prepare-send"),
            json!({}),
        )
        .await;
    assert_eq!(ready.status, 200, "{}", ready.text());
    let body = ready.json();
    assert_eq!(body["email"], json!(own_email));
    assert_eq!(body["whatsapp"], "+81 90 1234 5678");
    assert_eq!(body["approved"], true);
    let defaults = app
        .get(
            &sa.token,
            &format!("/api/v1/documents/quotation/{own}/send-defaults"),
        )
        .await
        .json();
    assert_eq!(defaults, body);
}

#[tokio::test]
async fn test_approved_quotation_is_emailed_with_its_verified_pdf() {
    let (app, _g) = setup().await;
    let bdr = c::make_person(app, &["sales_bizdev"], SEND_PERMS).await;
    let (qid, email) = quote(app, true).await; // catalog at list price: auto-approved
    let r = send(
        app,
        &bdr.token,
        qid,
        json!({"channel": "email", "to": [email], "cc": ["boss@customer.test"], "message": "Hi Bob"}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let verify_url = r.json()["verify_url"].as_str().unwrap().to_string();
    assert!(verify_url.rsplit_once('/').unwrap().0.ends_with("/v"));

    let mails = EMAILS.lock().unwrap().clone();
    assert_eq!(mails.len(), 1);
    let mail = &mails[0];
    assert_eq!(mail.cfg.from_email, "sales@example.test");
    assert_eq!(mail.to, std::slice::from_ref(&email));
    assert_eq!(
        mail.cc.as_deref(),
        Some(&["boss@customer.test".to_string()][..])
    );
    let atts = mail.attachments.as_ref().unwrap();
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0].mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(atts[0].content, c::PDF);
    assert!(mail.html_body.contains(&verify_url) && mail.html_body.contains("Hi Bob"));
    let mut db = app.db().await;
    assert_eq!(
        c::status_of(&mut db, "quotations", qid).await.as_deref(),
        Some("sent")
    );
    let (kind, direction): (String, Option<String>) = sqlx::query_as(
        "SELECT activity_type, direction FROM activities WHERE channel_data->>'document_id' = $1",
    )
    .bind(qid.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (kind.as_str(), direction.as_deref()),
        ("email", Some("outbound"))
    );
}

#[tokio::test]
async fn test_approved_quotation_goes_by_whatsapp_as_a_pdf() {
    let (app, _g) = setup().await;
    let bdr = c::make_person(app, &["sales_bizdev"], SEND_PERMS).await;
    let (qid, _) = quote(app, true).await;
    let r = send(
        app,
        &bdr.token,
        qid,
        json!({"channel": "whatsapp", "to": ["+81 90 1234 5678"]}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
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
        .contains(r.json()["verify_url"].as_str().unwrap()));
}

#[tokio::test]
async fn test_sending_needs_the_send_permission() {
    let (app, _g) = setup().await;
    let viewer = c::make_person(app, &[], &[]).await;
    let (qid, email) = quote(app, true).await;
    let r = send(
        app,
        &viewer.token,
        qid,
        json!({"channel": "email", "to": [email]}),
    )
    .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json(),
        json!({"detail": "Permission required: sales:send_quotation"})
    );
    assert!(EMAILS.lock().unwrap().is_empty());
}

async fn send_invoice(
    app: &support::TestApp,
    token: &str,
    iid: Uuid,
    body: Value,
) -> support::Resp {
    app.post(
        token,
        &format!("/api/v1/documents/invoice/{iid}/send"),
        body,
    )
    .await
}

#[tokio::test]
async fn test_an_invoice_for_a_customer_outside_the_crm_prefills_its_billing_details() {
    let (app, _g) = setup().await;
    let sa = c::owner(app).await;
    let mut db = app.db().await;
    let iid = c::make_invoice(&mut db, "ap@walkin.test", "+81 90 7777 0000").await;
    let prep = app
        .post(
            &sa.token,
            &format!("/api/v1/documents/invoice/{iid}/prepare-send"),
            json!({}),
        )
        .await;
    assert_eq!(prep.status, 200, "{}", prep.text());
    let body = prep.json();
    assert_eq!(
        (&body["name"], &body["email"], &body["whatsapp"]),
        (
            &json!("Walk-in Co"),
            &json!("ap@walkin.test"),
            &json!("+81 90 7777 0000")
        )
    );
}

#[tokio::test]
async fn test_a_second_send_to_the_same_recipient_needs_confirmation() {
    let (app, _g) = setup().await;
    let sa = c::owner(app).await;
    let mut db = app.db().await;
    let email = format!("{}@cust.test", support::u("sarah"));
    let iid = c::make_invoice(&mut db, &email, "+81 80 1111 2222").await;
    let first = send_invoice(
        app,
        &sa.token,
        iid,
        json!({"channel": "email", "to": [email]}),
    )
    .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let again = send_invoice(
        app,
        &sa.token,
        iid,
        json!({"channel": "email", "to": [email]}),
    )
    .await;
    assert_eq!(again.status, 409, "{}", again.text());
    assert_eq!(again.json()["detail"]["code"], "recently_sent");
    assert!(again.json()["detail"]["sent_at"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(EMAILS.lock().unwrap().len(), 1);
    let other = send_invoice(
        app,
        &sa.token,
        iid,
        json!({"channel": "email", "to": ["someone.else@cust.test"]}),
    )
    .await;
    assert_eq!(other.status, 200, "{}", other.text());
    let forced = send_invoice(
        app,
        &sa.token,
        iid,
        json!({"channel": "email", "to": [email], "force": true}),
    )
    .await;
    assert_eq!(forced.status, 200, "{}", forced.text());
    assert_eq!(EMAILS.lock().unwrap().len(), 3);
    assert_eq!(SENT_FOLDER.lock().unwrap().len(), 3); // a copy of each went to the Sent folder
    assert_eq!(
        c::status_of(&mut db, "invoices", iid).await.as_deref(),
        Some("sent")
    );
}

#[tokio::test]
async fn test_a_send_already_in_progress_is_not_started_twice() {
    let (app, _g) = setup().await;
    let sa = c::owner(app).await;
    let (qid, email) = quote(app, false).await;
    c::approve_as_owner(app, "quotation", qid).await;
    let key = granary_services::verification::delivery::send_key_for_id(
        "quotation",
        &qid.to_string(),
        "email",
        std::slice::from_ref(&email),
    );
    let client = redis::Client::open(app.redis_url.as_str()).unwrap();
    let mut r = client.get_multiplexed_async_connection().await.unwrap();
    let _: () = redis::cmd("SET")
        .arg(format!("{key}:inflight"))
        .arg("1")
        .query_async(&mut r)
        .await
        .unwrap();
    let res = send(
        app,
        &sa.token,
        qid,
        json!({"channel": "email", "to": [email], "force": true}),
    )
    .await;
    assert_eq!(res.status, 409, "{}", res.text());
    assert_eq!(res.json()["detail"]["code"], "send_in_progress");
    assert!(EMAILS.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_a_pdf_render_failure_is_explained_and_nothing_is_sent() {
    let (app, _g) = setup().await;
    let sa = c::owner(app).await;
    let (qid, email) = quote(app, false).await;
    c::approve_as_owner(app, "quotation", qid).await;
    c::PDF_DOWN.store(true, Ordering::SeqCst);
    let r = send(
        app,
        &sa.token,
        qid,
        json!({"channel": "email", "to": [email.clone()]}),
    )
    .await;
    c::PDF_DOWN.store(false, Ordering::SeqCst);
    // 10 Nov 2026: 503 (Cloudflare replaces an origin 502 with its own page), with the reason
    assert_eq!(r.status, 503, "{}", r.text());
    let d = r.json()["detail"].as_str().unwrap().to_string();
    assert!(
        d.contains("could not be rendered")
            && d.contains("the PDF service answered with error 503"),
        "{d}"
    );
    assert!(EMAILS.lock().unwrap().is_empty() && c::WA_SENT.lock().unwrap().is_empty());
    // the lock was released: the next send goes through
    let ok = send(
        app,
        &sa.token,
        qid,
        json!({"channel": "email", "to": [email]}),
    )
    .await;
    assert_eq!(ok.status, 200, "{}", ok.text());
}

/// Sending a quotation a second time (no Python test; the outcome follows Python's order in
/// `send_document`: `ensure_or_submit` runs before the resend guard). The first send gives a quotation
/// without a full number its `{number}-{revision}` (`_mark_sent`), which changes the approved facts
/// ("number"), so the approval is stale, the resubmission refuses a 'sent' quotation: 400. A quotation
/// that already had its full number keeps its approval and meets the resend guard: 409 recently_sent.
#[tokio::test]
async fn test_a_quotation_sent_twice() {
    let (app, _g) = setup().await;
    let bdr = c::make_person(app, &["sales_bizdev"], SEND_PERMS).await;
    let (qid, email) = quote(app, true).await; // catalog at list price: auto-approved
    let body = json!({"channel": "email", "to": [email]});
    let first = send(app, &bdr.token, qid, body.clone()).await;
    assert_eq!(first.status, 200, "{}", first.text());
    let again = send(app, &bdr.token, qid, body).await;
    assert_eq!(again.status, 400, "{}", again.text());
    assert_eq!(
        again.json(),
        json!({"detail": "A quotation with status 'sent' cannot be submitted for approval"})
    );

    let (qid, email) = quote(app, true).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE quotations SET full_number = number || '-1' WHERE id = $1")
        .bind(qid)
        .execute(&mut db)
        .await
        .unwrap();
    let body = json!({"channel": "email", "to": [email]});
    let first = send(app, &bdr.token, qid, body.clone()).await;
    assert_eq!(first.status, 200, "{}", first.text());
    let again = send(app, &bdr.token, qid, body).await;
    assert_eq!(again.status, 409, "{}", again.text());
    assert_eq!(again.json()["detail"]["code"], "recently_sent");
    assert_eq!(EMAILS.lock().unwrap().len(), 2);
}
