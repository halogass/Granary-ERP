//! routers/portal_auth.py through the real app. The HTTP-only tests live in compat-tests/http-suite
//! (test_portal_auth_registrations.py); this file holds the one that replaces the email service.

mod support;

use std::sync::{Arc, Mutex};

use futures::FutureExt;
use serde_json::json;
use support::{app, UserSpec};
use uuid::Uuid;

use granary_services::email::{set_global_seams, Seams, SmtpConfig};

const TAG: &str = "portal_auth";

async fn add_registration(
    app: &support::TestApp,
    company: &str,
    contact: &str,
    email: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO portal_registration_requests (id, portal_type, status, company_name, contact_name, email) \
         VALUES ($1, 'vendor', 'pending', $2, $3, $4)",
    )
    .bind(id)
    .bind(company)
    .bind(contact)
    .bind(email)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

/// (recipients, subject, html) of a captured send.
type SentMail = (Vec<String>, String, String);

/// backend/tests/test_audit_low_w2.py::test_portal_approval_reports_a_failed_welcome_email
#[tokio::test]
async fn test_portal_approval_reports_a_failed_welcome_email() {
    let app = app(TAG);
    let _g = app.serial().await;
    let admin = app.make_user(UserSpec::new().superadmin()).await;
    let sent_html: Arc<Mutex<Vec<SentMail>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = sent_html.clone();
    set_global_seams(Some(Seams {
        load_system_smtp: Some(Arc::new(|| {
            async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    from_email: "no-reply@granary.test".into(),
                    from_name: "ACME".into(),
                    ..Default::default()
                })
            }
            .boxed()
        })),
        send_via_config: Some(Arc::new(move |call| {
            sink.lock().unwrap().push((
                call.to.clone(),
                call.subject.clone(),
                call.html_body.clone(),
            ));
            async { Ok((false, None)) }.boxed()
        })),
        load_crm_smtp: None,
    }));

    let email = format!(
        "Pytest.Portal.{}@Acme.Test",
        &Uuid::new_v4().simple().to_string()[..6]
    );
    let reg = add_registration(app, "<b>Evil</b> Co", "<script>x</script>", &email).await;
    let r = app
        .post(
            &admin.token,
            &format!("/api/v1/portal/registrations/{reg}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let created_user = body["user_id"].as_str().unwrap().to_string();
    assert_eq!(body["email_sent"], json!(false));
    assert!(body["message"].as_str().unwrap().contains("NOT sent"));
    assert_eq!(
        body["email_error"],
        json!("the mail server rejected or did not accept the message")
    );
    {
        let sent = sent_html.lock().unwrap();
        let (to, subject, html) = &sent[0];
        assert!(!html.contains("<script>") && html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;b&gt;Evil&lt;/b&gt; Co"));
        assert_eq!(to, &vec![email.to_lowercase()]);
        assert_eq!(subject, "Your Vendor Portal Access - Granary");
    }
    let mut db = app.db().await;
    let (stored, vendor_id): (String, Option<Uuid>) =
        sqlx::query_as("SELECT email, vendor_id FROM users WHERE id = $1::uuid")
            .bind(&created_user)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(stored, email.to_lowercase());
    let reg_email: String =
        sqlx::query_scalar("SELECT email FROM portal_registration_requests WHERE id = $1")
            .bind(reg)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        reg_email,
        email.to_lowercase(),
        "the registration's address is normalised too"
    );
    assert!(vendor_id.is_some());

    // The same address in another case is a duplicate
    let reg2 = add_registration(app, "Other", "Other", &email.to_uppercase()).await;
    let r = app
        .post(
            &admin.token,
            &format!("/api/v1/portal/registrations/{reg2}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 409);
    set_global_seams(None);
}

/// A sending error (not a refusal) is reported as "sending failed"; a successful send says so.
#[tokio::test]
async fn approval_reports_send_errors_and_success() {
    let app = app(TAG);
    let _g = app.serial().await;
    let admin = app.make_user(UserSpec::new().superadmin()).await;
    let outcome = Arc::new(Mutex::new(0u8));
    let o = outcome.clone();
    set_global_seams(Some(Seams {
        load_system_smtp: Some(Arc::new(|| {
            async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    from_email: "no-reply@granary.test".into(),
                    ..Default::default()
                })
            }
            .boxed()
        })),
        send_via_config: Some(Arc::new(move |_call| {
            let n = *o.lock().unwrap();
            async move {
                if n == 0 {
                    Err(granary_services::email::EmailError::Smtp(
                        "connection refused".into(),
                    ))
                } else {
                    Ok((true, None))
                }
            }
            .boxed()
        })),
        load_crm_smtp: None,
    }));
    let e1 = format!(
        "pytest.p1.{}@example.com",
        &Uuid::new_v4().simple().to_string()[..6]
    );
    let reg = add_registration(app, "Co", "Ann", &e1).await;
    let r = app
        .post(
            &admin.token,
            &format!("/api/v1/portal/registrations/{reg}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["email_error"], json!("sending failed"));

    *outcome.lock().unwrap() = 1;
    let e2 = format!(
        "pytest.p2.{}@example.com",
        &Uuid::new_v4().simple().to_string()[..6]
    );
    let reg = add_registration(app, "Co", "Ann", &e2).await;
    let r = app
        .post(
            &admin.token,
            &format!("/api/v1/portal/registrations/{reg}/approve"),
            json!({}),
        )
        .await;
    let body = r.json();
    assert_eq!(
        body,
        json!({
            "message": "Registration approved. Credentials sent to applicant.",
            "user_id": body["user_id"],
            "email_sent": true,
            "email_error": null,
        })
    );
    set_global_seams(None);
}
