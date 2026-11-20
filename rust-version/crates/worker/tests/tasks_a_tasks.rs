//! W5-A's tasks (src/tasks/group_a.rs) run the way a Python test runs them
//! (`monkeypatch.setattr(tasks, "run_async", lambda coro: coro)` then `await tasks.x()`).
//!
//! Ported Python tests:
//! - backend/tests/test_audit_critical.py test_invoice_reminder_days_follow_the_settings.
//!
//! The other tests pin each task's return values and row effects (tasks.py has no Python test for them).

#![allow(clippy::type_complexity)]

mod support;

use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_worker::registry::{default_registry, Outcome};
use granary_worker::tasks::group_a::_reminder_due;
use support::{env, u, TestEnv};

const TAG: &str = "tasks_a";
const REDIS_DB: u32 = 8;

fn the_env() -> &'static TestEnv {
    env(TAG, REDIS_DB)
}

fn done(v: impl Into<Value>) -> Outcome {
    Outcome::Done(v.into())
}

async fn org(db: &mut PgConnection, email: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, email, is_deleted) VALUES ($1, $2, $3, false)",
    )
    .bind(id)
    .bind(u("Pytest Org"))
    .bind(email)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn set_setting(db: &mut PgConnection, key: &str, value: Option<&str>) -> Option<String> {
    let old: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await
            .unwrap();
    match value {
        None => {
            sqlx::query("DELETE FROM system_settings WHERE key = $1")
                .bind(key)
                .execute(&mut *db)
                .await
                .unwrap();
        }
        Some(v) if old.is_some() => {
            sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
                .bind(key)
                .bind(v)
                .execute(&mut *db)
                .await
                .unwrap();
        }
        Some(v) => {
            sqlx::query(
                "INSERT INTO system_settings (id, key, value, value_type, category, label, is_public, is_encrypted) \
                 VALUES ($1, $2, $3, 'string', 'test', 'test', false, false)",
            )
            .bind(Uuid::new_v4())
            .bind(key)
            .bind(v)
            .execute(&mut *db)
            .await
            .unwrap();
        }
    }
    old.flatten()
}

async fn restore_setting(db: &mut PgConnection, key: &str, old: Option<String>, vtype: &str) {
    match old {
        Some(v) => {
            if set_setting(db, key, Some(&v)).await.is_none() {
                sqlx::query("UPDATE system_settings SET value_type = $2 WHERE key = $1")
                    .bind(key)
                    .bind(vtype)
                    .execute(&mut *db)
                    .await
                    .unwrap();
            }
        }
        None => {
            set_setting(db, key, None).await;
        }
    }
}

async fn call(e: &TestEnv, name: &str, args: Vec<Value>) -> Outcome {
    e.call(name, args, 0).await.expect("task ran")
}

// ─── C1 · invoice reminder schedule ──────────────────────────────────────────

#[test]
fn test_invoice_reminder_days_follow_the_settings() {
    let pick = |n: i64, s: &[i64], r: i64| -> Vec<i64> {
        (1..n).filter(|d| _reminder_due(*d, s, r)).collect()
    };
    assert_eq!(pick(95, &[1, 7, 14, 30], 30), vec![1, 7, 14, 30, 60, 90]);
    assert_eq!(pick(30, &[3, 10], 7), vec![3, 10, 17, 24]);
    assert_eq!(pick(60, &[5], 0), vec![5]); // 0 = no repeats
}

// ─── Invoice reminders ───────────────────────────────────────────────────────

fn mail_seams(
    calls: Arc<Mutex<Vec<granary_services::email::SendCall>>>,
) -> granary_services::email::Seams {
    use granary_services::email::{Seams, SmtpConfig};
    Seams {
        send_via_config: Some(Arc::new(move |call| {
            calls.lock().unwrap().push(call);
            Box::pin(async { Ok((true, None)) })
        })),
        load_system_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: String::new(),
                    password: String::new(),
                    from_name: "Granary".into(),
                    from_email: "noreply@granary.test".into(),
                    tls: false,
                })
            })
        })),
        load_crm_smtp: None,
    }
}

/// `tasks.name(*args)` with email seams (task-local to this call).
async fn call_with_mail(
    e: &TestEnv,
    name: &str,
    args: Vec<Value>,
    seams: granary_services::email::Seams,
) -> Outcome {
    let ctx = e.ctx(name, args, Map::new(), 0);
    let def = *default_registry().get(name).unwrap();
    e.run(granary_services::email::with_seams(
        seams,
        granary_services::approvals::notify::scope((def.run)(ctx)),
    ))
    .await
    .expect("task ran")
}

async fn invoice(
    db: &mut PgConnection,
    org_id: Uuid,
    status: &str,
    due: NaiveDate,
    amount_due: &str,
) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let number = u("PYINV");
    sqlx::query(&format!(
        "INSERT INTO invoices (id, number, full_number, invoice_type, customer_org_id, customer_name, currency, \
         status, issue_date, due_date, subtotal, total, amount_paid, amount_due, refund_due, is_deleted) \
         VALUES ($1, $2, $3, 'commercial', $4, 'Pytest <Customer>', 'JPY', $5, $6, $6, {amount_due}, \
         {amount_due}, 0, {amount_due}, 0, false)"
    ))
    .bind(id)
    .bind(&number)
    .bind(format!("{number}-1"))
    .bind(org_id)
    .bind(status)
    .bind(due)
    .execute(&mut *db)
    .await
    .unwrap();
    (id, format!("{number}-1"))
}

#[tokio::test]
async fn overdue_invoice_reminders_follow_the_schedule_and_mark_sent_invoices_overdue() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    // other tests' rows must not be overdue here
    sqlx::query("UPDATE invoices SET is_deleted = true WHERE is_deleted = false")
        .execute(&mut db)
        .await
        .unwrap();
    let old_days = set_setting(&mut db, "finance.invoice_reminder_days", Some("[3]")).await;
    let old_repeat = set_setting(&mut db, "finance.invoice_reminder_repeat_days", Some("0")).await;
    let company_today = granary_core::clock::today_local(&mut db).await.unwrap();
    let places = granary_core::settings_store::currency_places(&mut db, Some("JPY"))
        .await
        .unwrap();

    let billed = org(&mut db, Some("billing@customer.test")).await;
    let no_mail = org(&mut db, None).await;
    let due = company_today - TimeDelta::days(3);
    let (due_id, due_number) = invoice(&mut db, billed, "sent", due, "1500000").await;
    let (_, _) = invoice(&mut db, no_mail, "overdue", due, "10").await;
    // 2 days overdue: not a reminder day, but its status still becomes overdue
    let (early_id, _) = invoice(
        &mut db,
        billed,
        "sent",
        company_today - TimeDelta::days(2),
        "10",
    )
    .await;

    let calls = Arc::new(Mutex::new(Vec::new()));
    let out = call_with_mail(
        e,
        "app.tasks.send_invoice_reminders",
        vec![],
        mail_seams(calls.clone()),
    )
    .await;
    assert_eq!(out, done("Sent 1 overdue invoice reminder(s), skipped 1"));
    let sent = calls.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, vec!["billing@customer.test".to_string()]);
    assert_eq!(
        sent[0].subject,
        format!("Reminder: invoice {due_number} is 3 day(s) overdue")
    );
    let amount = if places == 0 {
        "1,500,000".to_string()
    } else {
        format!("1,500,000.{}", "0".repeat(places as usize))
    };
    assert_eq!(
        sent[0].html_body,
        format!(
            "<p>Dear Pytest &lt;Customer&gt;,</p><p>Our records show invoice <strong>{due_number}</strong> for \
             JPY {amount} became due on {due} (3 day(s) ago).</p><p>If payment is already on its way, \
             please ignore this reminder. Otherwise kindly arrange settlement or reply to this email with a \
             date.</p><p>Thank you,<br>{}</p>",
            granary_compat::pyfmt::html_escape(
                &granary_core::company::get_company_info(&mut db)
                    .await
                    .unwrap()
                    .company_name,
                true
            )
        )
    );
    for id in [due_id, early_id] {
        let status: String = sqlx::query_scalar("SELECT status FROM invoices WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
        assert_eq!(status, "overdue");
    }

    // A broken schedule setting skips the run (and changes nothing).
    set_setting(&mut db, "finance.invoice_reminder_days", Some("[0, 7]")).await;
    let out = call_with_mail(
        e,
        "app.tasks.send_invoice_reminders",
        vec![],
        mail_seams(calls.clone()),
    )
    .await;
    assert_eq!(
        out,
        done(
            "Invoice reminders skipped: Setting 'finance.invoice_reminder_days' must be a list of positive whole \
             days, e.g. [1, 7]. Set it in Settings \u{2192} Money & tax \u{2192} Invoices."
        )
    );

    sqlx::query("UPDATE invoices SET is_deleted = true WHERE is_deleted = false")
        .execute(&mut db)
        .await
        .unwrap();
    let out = call(e, "app.tasks.send_invoice_reminders", vec![]).await;
    assert_eq!(out, done("No overdue invoices to remind about"));
    restore_setting(&mut db, "finance.invoice_reminder_days", old_days, "json").await;
    restore_setting(
        &mut db,
        "finance.invoice_reminder_repeat_days",
        old_repeat,
        "int",
    )
    .await;
}

#[tokio::test]
async fn send_document_email_attaches_the_downloaded_pdf() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let e = the_env();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/doc.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.4 test".to_vec()))
        .mount(&server)
        .await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let out = call_with_mail(
        e,
        "app.tasks.send_document_email",
        vec![
            json!("client@example.com"),
            json!("Your quotation"),
            json!("<p>Hi</p>"),
            json!(format!("{}/doc.pdf", server.uri())),
            json!("Q-1.pdf"),
        ],
        mail_seams(calls.clone()),
    )
    .await;
    assert_eq!(out, done(true));
    let sent = calls.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Your quotation");
    let atts = sent[0].attachments.clone().expect("attachment");
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0].filename, "Q-1.pdf");
    assert_eq!(atts[0].content, b"%PDF-1.4 test".to_vec());

    // without pdf_url: no attachments (None)
    let out = call_with_mail(
        e,
        "app.tasks.send_document_email",
        vec![json!("client@example.com"), json!("S"), json!("<p>b</p>")],
        mail_seams(calls.clone()),
    )
    .await;
    assert_eq!(out, done(true));
    assert!(calls.lock().unwrap()[1].attachments.is_none());
}

// ─── Knowledge base ──────────────────────────────────────────────────────────

async fn article(db: &mut PgConnection, attachment: Option<Uuid>, status: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    let slug = u("pytest-article");
    sqlx::query(
        "INSERT INTO kb_articles (id, title, slug, content, status, is_private, is_deleted, \
         source_pdf_attachment_id, pdf_import_status) VALUES ($1, $2, $3, 'x', 'draft', false, false, $4, $5)",
    )
    .bind(id)
    .bind(&slug)
    .bind(&slug)
    .bind(attachment)
    .bind(status)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn import_state(db: &mut PgConnection, id: Uuid) -> (Option<String>, Option<String>) {
    sqlx::query_as("SELECT pdf_import_status, pdf_import_error FROM kb_articles WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn import_kb_article_pdf_records_each_outcome_on_the_article() {
    let e = the_env();
    let mut db = e.db().await;
    let out = call(
        e,
        "app.tasks.import_kb_article_pdf",
        vec![json!(Uuid::new_v4().to_string())],
    )
    .await;
    assert_eq!(out, done("KB PDF import: article not found"));

    let plain = article(&mut db, None, None).await;
    let out = call(
        e,
        "app.tasks.import_kb_article_pdf",
        vec![json!(plain.to_string())],
    )
    .await;
    assert_eq!(out, done("KB PDF import: no source PDF on this article"));

    // The PDF cannot be read (object store unreachable): an unexpected error, recorded on the article.
    let att = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, \
         file_type) VALUES ($1, 'kb_article', 'x', 'a.pdf', 10, 'application/pdf', $2, 'u', 'document')",
    )
    .bind(att)
    .bind(format!("kb/{att}.pdf"))
    .execute(&mut db)
    .await
    .unwrap();
    let art = article(&mut db, Some(att), Some("pending")).await;
    let out = call(
        e,
        "app.tasks.import_kb_article_pdf",
        vec![json!(art.to_string())],
    )
    .await;
    let Outcome::Done(Value::String(text)) = out else {
        panic!("{out:?}")
    };
    assert!(text.starts_with("KB PDF import error: "), "{text}");
    let (status, error) = import_state(&mut db, art).await;
    assert_eq!(status.as_deref(), Some("failed"));
    assert!(error.unwrap().starts_with("Unexpected error: "));

    // A malformed id: the task raises.
    assert!(e
        .call("app.tasks.import_kb_article_pdf", vec![json!("nope")], 0)
        .await
        .is_err());
}

#[tokio::test]
async fn sweep_stuck_pdf_imports_fails_old_pending_imports_only() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let old = article(&mut db, None, Some("processing")).await;
    let fresh = article(&mut db, None, Some("pending")).await;
    sqlx::query("UPDATE kb_articles SET updated_at = now() - interval '21 minutes' WHERE id = $1")
        .bind(old)
        .execute(&mut db)
        .await
        .unwrap();
    let out = call(e, "app.tasks.sweep_stuck_pdf_imports", vec![]).await;
    assert_eq!(out, done("KB PDF sweep: 1 stuck import(s) marked failed"));
    let (status, error) = import_state(&mut db, old).await;
    assert_eq!(status.as_deref(), Some("failed"));
    assert_eq!(
        error.as_deref(),
        Some(
            "The conversion never finished (no progress for over 20 minutes). The background worker may \
             have been unavailable. Use Re-convert to try again."
        )
    );
    assert_eq!(
        import_state(&mut db, fresh).await.0.as_deref(),
        Some("pending")
    );
    sqlx::query("UPDATE kb_articles SET pdf_import_status = NULL WHERE id = $1")
        .bind(fresh)
        .execute(&mut db)
        .await
        .unwrap();
    let out = call(e, "app.tasks.sweep_stuck_pdf_imports", vec![]).await;
    assert_eq!(out, done("KB PDF sweep: 0 stuck import(s) marked failed"));
}

#[tokio::test]
async fn reindex_and_index_report_a_missing_row() {
    let e = the_env();
    let out = call(
        e,
        "app.tasks.reindex_kb_embeddings",
        vec![json!(Uuid::new_v4().to_string())],
    )
    .await;
    assert_eq!(out, done("KB reindex: article not found"));
    let out = call(
        e,
        "app.tasks.index_ai_knowledge_doc",
        vec![json!(Uuid::new_v4().to_string())],
    )
    .await;
    assert_eq!(out, done("Document no longer exists"));
    assert!(e
        .call("app.tasks.index_ai_knowledge_doc", vec![json!("nope")], 0)
        .await
        .is_err());
}

#[tokio::test]
async fn index_ai_knowledge_doc_fails_when_the_pdf_cannot_be_read_back() {
    let e = the_env();
    let mut db = e.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_knowledge_docs (id, title, source_type, content, original_object_key, status, is_active, \
         customer_safe, chunk_count, use_count) VALUES ($1, 'Pytest PDF', 'pdf', '  ', $2, 'processing', true, \
         false, 0, 0)",
    )
    .bind(id)
    .bind(format!("ai-knowledge/{id}.pdf"))
    .execute(&mut db)
    .await
    .unwrap();
    let out = call(
        e,
        "app.tasks.index_ai_knowledge_doc",
        vec![json!(id.to_string())],
    )
    .await;
    assert_eq!(out, done("failed"));
    let (status, error): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM ai_knowledge_docs WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert!(error
        .unwrap()
        .starts_with("The uploaded file could not be read back: "));
}

// ─── Once-a-day / once-a-month guards ────────────────────────────────────────

#[tokio::test]
async fn expire_quotations_runs_once_per_company_day() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    set_setting(&mut db, "sales.quotation.last_expiry_pass", None).await;
    let out = call(e, "app.tasks.expire_quotations", vec![]).await;
    let Outcome::Done(Value::String(text)) = out else {
        panic!("{out:?}")
    };
    assert!(text.starts_with("Expired "), "{text}");
    let today = granary_core::clock::today_local(&mut db).await.unwrap();
    let (value, category, label): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT value, category, label FROM system_settings WHERE key = 'sales.quotation.last_expiry_pass'",
        )
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(value, Some(today.to_string()));
    assert_eq!(category.as_deref(), Some("sales"));
    assert_eq!(
        label.as_deref(),
        Some("Internal marker: last quotation expiry pass")
    );
    let out = call(e, "app.tasks.expire_quotations", vec![]).await;
    assert_eq!(out, done("Expiry already ran today"));
}

// ─── Approvals / signatures / mail / FX ──────────────────────────────────────

#[tokio::test]
async fn reminder_tasks_report_their_counts() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let out = call(e, "app.tasks.remind_overdue_approvals", vec![]).await;
    assert_eq!(out, done("Reminded 0 overdue approval(s)"));
    let out = call(e, "app.tasks.remind_expiring_signatures", vec![]).await;
    assert_eq!(out, done("Sent 0 signature deadline reminder(s)"));
    let old = set_setting(&mut db, "sign.reminder_window_hours", None).await;
    let out = call(e, "app.tasks.remind_expiring_signatures", vec![]).await;
    restore_setting(&mut db, "sign.reminder_window_hours", old, "int").await;
    assert_eq!(
        out,
        done(
            "Signature reminders skipped: Setting 'sign.reminder_window_hours' is not set. Set it in Settings \
             \u{2192} Approvals & signing \u{2192} E-signing."
        )
    );
    let out = call(e, "app.tasks.sync_personal_mailboxes", vec![]).await;
    assert_eq!(out, done("Synced 0 mailbox(es)"));
}
