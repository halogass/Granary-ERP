//! W5-A's tasks (docs/WAVE5_BRIEF.md): send_invoice_reminders, sync_exchange_rates, send_document_email, reindex_kb_embeddings,
//! import_kb_article_pdf, sweep_stuck_pdf_imports, remind_overdue_approvals, remind_expiring_signatures,
//! sync_personal_mailboxes, index_ai_knowledge_doc, expire_quotations (with `_reminder_due` and
//! `_reminder_schedule`).

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyfmt::{format_number, html_escape, PyNum};
use granary_core::db::DbSession;
use granary_core::errors::ApiError;
use granary_core::settings_store::{
    currency_places, require_int, require_json, SettingError, StoreError,
};
use granary_db::rows::ai::AIKnowledgeDoc;
use granary_db::rows::finance::Invoice;
use granary_db::rows::knowledge::KBArticle;

use crate::registry::{Outcome, Param, TaskCtx, TaskDef, TaskResult};
use crate::tasks::common::{_already_ran, _mark_ran, head};
use crate::tasks::err;

pub const TASKS: &[TaskDef] = &[
    TaskDef::new("app.tasks.send_invoice_reminders", &[], |ctx| {
        Box::pin(send_invoice_reminders(ctx))
    }),
    TaskDef::new("app.tasks.sync_exchange_rates", &[], |ctx| {
        Box::pin(sync_exchange_rates(ctx))
    }),
    TaskDef::new(
        "app.tasks.send_document_email",
        &[
            Param::req("to"),
            Param::req("subject"),
            Param::req("html_body"),
            Param::opt("pdf_url", "null"),
            Param::opt("filename", "null"),
        ],
        |ctx| Box::pin(send_document_email(ctx)),
    ),
    TaskDef::new(
        "app.tasks.reindex_kb_embeddings",
        &[Param::opt("article_id", "null")],
        |ctx| Box::pin(reindex_kb_embeddings(ctx)),
    ),
    TaskDef::new(
        "app.tasks.import_kb_article_pdf",
        &[Param::req("article_id")],
        |ctx| Box::pin(import_kb_article_pdf(ctx)),
    ),
    TaskDef::new("app.tasks.sweep_stuck_pdf_imports", &[], |ctx| {
        Box::pin(sweep_stuck_pdf_imports(ctx))
    }),
    TaskDef::new("app.tasks.remind_overdue_approvals", &[], |ctx| {
        Box::pin(remind_overdue_approvals(ctx))
    }),
    TaskDef::new("app.tasks.remind_expiring_signatures", &[], |ctx| {
        Box::pin(remind_expiring_signatures(ctx))
    }),
    TaskDef::new("app.tasks.sync_personal_mailboxes", &[], |ctx| {
        Box::pin(sync_personal_mailboxes(ctx))
    }),
    TaskDef::new(
        "app.tasks.index_ai_knowledge_doc",
        &[Param::req("doc_id")],
        |ctx| Box::pin(index_ai_knowledge_doc(ctx)),
    ),
    TaskDef::new("app.tasks.expire_quotations", &[], |ctx| {
        Box::pin(expire_quotations(ctx))
    }),
    // doclib A1 (Rust only): review reminders of library documents
    TaskDef::new("app.tasks.remind_document_reviews", &[], |ctx| {
        Box::pin(remind_document_reviews(ctx))
    }),
    // fix-longapply (Rust only): the stuck-card watchdog
    TaskDef::new("app.tasks.nary_fail_stuck_cards", &[], |ctx| {
        Box::pin(nary_fail_stuck_cards(ctx))
    }),
    // 3 Nov 2026 (Rust only): the work queue's WhatsApp nudge to the Owner
    TaskDef::new("app.tasks.nary_work_queue_nudge", &[], |ctx| {
        Box::pin(nary_work_queue_nudge(ctx))
    }),
    // 17 Nov 2026 (Rust only): R&D backlog items <-> GitLab issues
    TaskDef::new("app.tasks.gitlab_issue_sync_tick", &[], |ctx| {
        Box::pin(gitlab_issue_sync_tick(ctx))
    }),
];

/// A settings-store error as the task would raise it.
fn store_err(e: StoreError) -> anyhow::Error {
    err(ApiError::from(e))
}

/// `SettingError` raised through a service that returns ApiError (it becomes a 400 whose detail starts
/// "Setting '"): the detail, so `except SettingError as exc` can be reproduced.
fn setting_detail(e: &ApiError) -> Option<String> {
    match e {
        ApiError::Http {
            status,
            detail: Value::String(s),
            ..
        } if status.as_u16() == 400 && s.starts_with("Setting '") => Some(s.clone()),
        _ => None,
    }
}

/// `getattr(e, "detail", None) or str(e)` for an exception from a router helper.
fn detail_or_str(e: &ApiError) -> String {
    let detail = match e {
        ApiError::Http { detail, .. } => match detail {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            Value::Bool(false) => String::new(),
            Value::Array(a) if a.is_empty() => String::new(),
            Value::Object(o) if o.is_empty() => String::new(),
            other => {
                granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other))
            }
        },
        ApiError::App { detail, .. } => detail.clone(),
        _ => String::new(),
    };
    if detail.is_empty() {
        e.to_string()
    } else {
        detail
    }
}

/// `uuid.UUID(...)` / a string compared with a UUID column: a malformed id raises.
fn parse_id(v: &str) -> Result<Uuid, anyhow::Error> {
    granary_compat::text::utils::py_uuid(v)
        .ok_or_else(|| anyhow::anyhow!("badly formed hexadecimal UUID string"))
}

// ─── Invoice reminders ───────────────────────────────────────────────────────

/// `_reminder_due(days_overdue, schedule, repeat_every)`: overdue reminders go out on the days in
/// `schedule` (finance.invoice_reminder_days), then every `repeat_every` days after the last of them
/// (finance.invoice_reminder_repeat_days; 0 = never).
pub fn _reminder_due(days_overdue: i64, schedule: &[i64], repeat_every: i64) -> bool {
    if schedule.contains(&days_overdue) {
        return true;
    }
    let last = schedule.iter().copied().max().unwrap_or(0);
    repeat_every != 0 && days_overdue > last && (days_overdue - last).rem_euclid(repeat_every) == 0
}

/// `_reminder_schedule(db)`: (sorted reminder days, repeat interval) from the finance settings.
pub async fn _reminder_schedule(db: &mut PgConnection) -> Result<(Vec<i64>, i64), StoreError> {
    let where_ = "Money & tax \u{2192} Invoices";
    let days = require_json(db, "finance.invoice_reminder_days", where_).await?;
    // `isinstance(d, int) and d > 0` (a bool is an int in Python: true counts as 1)
    let parsed: Option<Vec<i64>> = match &days {
        Value::Array(items) => items
            .iter()
            .map(|d| match d {
                Value::Bool(true) => Some(1),
                Value::Number(n) => n.as_i64().filter(|v| *v > 0),
                _ => None,
            })
            .collect(),
        _ => None,
    };
    let Some(mut days) = parsed else {
        return Err(SettingError::new(
            "finance.invoice_reminder_days",
            "must be a list of positive whole days, e.g. [1, 7]",
            where_,
        )
        .into());
    };
    days.sort_unstable();
    let repeat = require_int(db, "finance.invoice_reminder_repeat_days", where_).await?;
    Ok((days, repeat))
}

/// `send_invoice_reminders()`: email the billing contact of every overdue, still-unpaid invoice.
pub async fn send_invoice_reminders(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let mut conn = db.conn().await.map_err(err)?;
    let today = granary_core::clock::today_local(&mut conn)
        .await
        .map_err(store_err)?;
    let overdue: Vec<Invoice> = sqlx::query_as(
        "SELECT * FROM invoices WHERE invoices.status IN ('sent', 'partially_paid', 'overdue') \
         AND invoices.invoice_type IN ('commercial', 'proforma') AND invoices.due_date < $1 \
         AND invoices.amount_due > 0 AND invoices.is_deleted = false",
    )
    .bind(today)
    .fetch_all(&mut *conn)
    .await
    .map_err(err)?;
    if overdue.is_empty() {
        return Outcome::done("No overdue invoices to remind about");
    }
    let (schedule, repeat_every) = match _reminder_schedule(&mut conn).await {
        Ok(v) => v,
        Err(StoreError::Setting(e)) => {
            tracing::error!(error = %e.detail, "invoice_reminders_not_configured");
            return Outcome::done(format!("Invoice reminders skipped: {}", e.detail));
        }
        Err(e) => return Err(store_err(e)),
    };
    let company = granary_core::company::get_company_info(&mut conn)
        .await
        .map_err(err)?;
    let (mut sent_count, mut skipped) = (0, 0);
    for invoice in overdue {
        let due_date = invoice.due_date.expect("filtered due_date < today");
        let days_overdue = (today - due_date).num_days();
        // Keep the invoice's status honest while we are here.
        if invoice.status.as_deref() == Some("sent") {
            sqlx::query(
                "UPDATE invoices SET status = 'overdue', updated_at = now() WHERE invoices.id = $1",
            )
            .bind(invoice.id)
            .execute(&mut *conn)
            .await
            .map_err(err)?;
        }
        if !_reminder_due(days_overdue, &schedule, repeat_every) {
            continue;
        }
        let mut email: Option<String> = None;
        if let Some(contact_id) = invoice.customer_contact_id {
            email =
                sqlx::query_scalar::<_, Option<String>>("SELECT email FROM contacts WHERE id = $1")
                    .bind(contact_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(err)?
                    .flatten();
        }
        if email.as_deref().unwrap_or("").is_empty() {
            if let Some(org_id) = invoice.customer_org_id {
                email = sqlx::query_scalar::<_, Option<String>>(
                    "SELECT email FROM organizations WHERE id = $1",
                )
                .bind(org_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(err)?
                .flatten();
            }
        }
        let Some(email) = email.filter(|e| !e.is_empty()) else {
            skipped += 1;
            continue;
        };

        let number = invoice
            .full_number
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| invoice.number.clone());
        let places = match currency_places(&mut conn, invoice.currency.as_deref()).await {
            Ok(p) => p,
            Err(StoreError::Setting(e)) => {
                tracing::error!(invoice = %number, error = %e.detail, "invoice_reminder_failed");
                skipped += 1;
                continue;
            }
            Err(e) => return Err(store_err(e)),
        };
        // 13 Nov 2026: formatted from the Decimal itself (it went through f64 before), so the e-mail shows
        // exactly the invoice's amount_due
        let amount = invoice.amount_due.unwrap_or_default();
        let amount_text = format_number(
            &PyNum::Decimal(granary_compat::pyjson::PyDecimal::from(amount)),
            &format!(",.{places}f"),
        )
        .map_err(err)?;
        let html = format!(
            "<p>Dear {},</p><p>Our records show invoice <strong>{}</strong> for {} {} became due on {} \
             ({} day(s) ago).</p><p>If payment is already on its way, please ignore this reminder. \
             Otherwise kindly arrange settlement or reply to this email with a date.</p>\
             <p>Thank you,<br>{}</p>",
            html_escape(
                invoice
                    .customer_name
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Customer"),
                true
            ),
            html_escape(&number, true),
            html_escape(invoice.currency.as_deref().unwrap_or(""), true),
            amount_text,
            granary_compat::pyjson::date_isoformat(due_date),
            days_overdue,
            html_escape(&company.company_name, true),
        );
        let subject = format!("Reminder: invoice {number} is {days_overdue} day(s) overdue");
        match granary_services::email::send_email(
            std::slice::from_ref(&email),
            &subject,
            &html,
            &mut conn,
            None,
            None,
        )
        .await
        {
            Ok(true) => sent_count += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                // one bad address must not stop the run
                tracing::error!(invoice = %number, error = %e, "invoice_reminder_failed");
                skipped += 1;
            }
        }
    }
    drop(conn);
    db.commit().await.map_err(err)?;
    Outcome::done(format!(
        "Sent {sent_count} overdue invoice reminder(s), skipped {skipped}"
    ))
}

// ─── FX / documents ──────────────────────────────────────────────────────────

/// `sync_exchange_rates()`: fetch live rates from ExchangeRate-API and upsert into exchange_rates.
pub async fn sync_exchange_rates(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let count = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::fx::fetch_and_store_rates(&mut conn)
            .await
            .map_err(err)?
    };
    // fetch_and_store_rates commits (inside the session it is a savepoint: commit the session).
    db.commit().await.map_err(err)?;
    Outcome::done(format!("FX sync complete: {count} pairs stored"))
}

/// `send_document_email(to, subject, html_body, pdf_url=None, filename=None)`: send an email with an
/// optional PDF attachment (downloaded from `pdf_url`). Returns send_email's bool.
pub async fn send_document_email(ctx: TaskCtx) -> TaskResult {
    let to = ctx.str_arg("to");
    let subject = ctx.str_arg("subject");
    let html_body = ctx.str_arg("html_body");
    let pdf_url = ctx.opt_str("pdf_url").filter(|s| !s.is_empty());
    let filename = ctx.opt_str("filename").filter(|s| !s.is_empty());
    let mut attachments = Vec::new();
    if let (Some(pdf_url), Some(filename)) = (pdf_url, filename) {
        // httpx.AsyncClient() defaults: 5 s timeout, redirects not followed, any status accepted.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(err)?;
        let resp = client.get(&pdf_url).send().await.map_err(err)?;
        let content = resp.bytes().await.map_err(err)?.to_vec();
        attachments.push(granary_services::email::Attachment {
            filename,
            content,
            mime_type: None,
        });
    }
    // send_email loads SMTP settings from the DB, so it needs a session.
    let db = ctx.session();
    let mut conn = db.conn().await.map_err(err)?;
    let ok = granary_services::email::send_email(
        std::slice::from_ref(&to),
        &subject,
        &html_body,
        &mut conn,
        None,
        if attachments.is_empty() {
            None
        } else {
            Some(&attachments)
        },
    )
    .await
    .map_err(err)?;
    Outcome::done(ok)
}

// ─── Knowledge base ──────────────────────────────────────────────────────────

async fn load_article(db: &DbSession, id: Uuid) -> Result<Option<KBArticle>, ApiError> {
    let mut conn = db.conn().await?;
    Ok(
        sqlx::query_as("SELECT * FROM kb_articles WHERE kb_articles.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// `reindex_kb_embeddings(article_id=None)`: re-embed KB/SOP articles for AI retrieval. If `article_id`
/// is given, only that one; otherwise the whole (published, non-private) corpus.
pub async fn reindex_kb_embeddings(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    if ctx.truthy("article_id") {
        let article_id = ctx.str_arg("article_id");
        let id = parse_id(&article_id)?;
        let Some(art) = load_article(&db, id).await.map_err(err)? else {
            return Outcome::done("KB reindex: article not found");
        };
        let n = {
            let mut conn = db.conn().await.map_err(err)?;
            granary_services::ai::embeddings::reindex_article(&mut conn, &art, None, None)
                .await
                .map_err(err)?
        };
        db.commit().await.map_err(err)?;
        return Outcome::done(format!("KB reindex: article {article_id} -> {n} chunks"));
    }
    let summary = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::ai::embeddings::reindex_all(&mut conn)
            .await
            .map_err(err)?
    };
    db.commit().await.map_err(err)?;
    Outcome::done(format!(
        "KB reindex: {} articles, {} chunks",
        summary["articles"], summary["chunks"]
    ))
}

/// How long one article's conversion may take before it is declared failed.
pub const PDF_IMPORT_TIMEOUT_SECONDS: u64 = 900;
/// How long an import may sit unclaimed/unfinished before the sweeper gives up on it.
pub const PDF_IMPORT_STALE_MINUTES: i64 = 20;

/// An exception that escaped `_run()` of import_kb_article_pdf: `type(exc).__name__` and `str(exc)`.
struct Crash {
    kind: &'static str,
    msg: String,
}

impl From<ApiError> for Crash {
    fn from(e: ApiError) -> Crash {
        let kind = match &e {
            ApiError::Integrity(_) => "IntegrityError",
            ApiError::Http { .. } => "HTTPException",
            ApiError::App { .. } => "AppException",
            _ => "DBAPIError",
        };
        Crash {
            kind,
            msg: match &e {
                ApiError::Internal(inner) => inner.to_string(),
                _ => detail_or_str(&e),
            },
        }
    }
}

impl From<sqlx::Error> for Crash {
    fn from(e: sqlx::Error) -> Crash {
        Crash {
            kind: "DBAPIError",
            msg: e.to_string(),
        }
    }
}

async fn set_import_outcome(
    db: &DbSession,
    id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<(), Crash> {
    {
        let mut conn = db.conn().await?;
        sqlx::query(
            "UPDATE kb_articles SET pdf_import_status = $2, pdf_import_error = $3, updated_at = now() \
             WHERE kb_articles.id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(error)
        .execute(&mut *conn)
        .await?;
    }
    db.commit().await?;
    Ok(())
}

/// `import_kb_article_pdf(article_id)`: convert an article's attached PDF into Markdown and write it into
/// the article body. The article stays a DRAFT: nothing is indexed for AI retrieval until a human reviews
/// and publishes it (publishing already queues the reindex).
pub async fn import_kb_article_pdf(ctx: TaskCtx) -> TaskResult {
    let article_id = ctx.str_arg("article_id");
    match import_run(&ctx, &article_id).await {
        Ok(s) => Outcome::done(s),
        Err(crash) => {
            // The task itself died (before or after the inner handlers could record anything). Without
            // this the article shows "converting..." until the sweeper notices.
            let reason = format!(
                "The conversion task failed unexpectedly: {}: {}",
                crash.kind,
                head(&crash.msg, 300)
            );
            let _ = mark_crashed(&ctx, &article_id, &reason).await;
            Err(anyhow::anyhow!("{}", crash.msg))
        }
    }
}

async fn import_run(ctx: &TaskCtx, article_id: &str) -> Result<String, Crash> {
    let id = granary_compat::text::utils::py_uuid(article_id).ok_or_else(|| Crash {
        kind: "DBAPIError",
        msg: format!("invalid input for query argument $1: '{article_id}' (invalid UUID)"),
    })?;
    let db = ctx.session();
    let Some(art) = load_article(&db, id).await? else {
        return Ok("KB PDF import: article not found".into());
    };
    let Some(att_id) = art.source_pdf_attachment_id else {
        return Ok("KB PDF import: no source PDF on this article".into());
    };
    let object_key: Option<String> = {
        let mut conn = db.conn().await?;
        sqlx::query_scalar("SELECT object_key FROM attachments WHERE attachments.id = $1")
            .bind(att_id)
            .fetch_optional(&mut *conn)
            .await?
    };
    let Some(object_key) = object_key else {
        set_import_outcome(
            &db,
            art.id,
            "failed",
            Some("The uploaded PDF is no longer available"),
        )
        .await?;
        return Ok("KB PDF import: attachment missing".into());
    };

    set_import_outcome(&db, art.id, "processing", None).await?;

    enum Fail {
        Timeout,
        Import(String),
        Other(String),
    }
    let converted: Result<Value, Fail> = async {
        let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
        let pdf_bytes = granary_services::storage::minio()
            .get_object(&bucket, &object_key)
            .await
            .map_err(|e| Fail::Other(e.to_string()))?;
        let mut conn = db.conn().await.map_err(|e| Fail::Other(e.to_string()))?;
        // A provider call that never returns would otherwise leave the article stuck on "processing".
        match tokio::time::timeout(
            Duration::from_secs(PDF_IMPORT_TIMEOUT_SECONDS),
            granary_services::ai::pdf_import::convert_pdf_to_markdown(
                &mut conn,
                &pdf_bytes,
                art.created_by_id,
            ),
        )
        .await
        {
            Err(_) => Err(Fail::Timeout),
            Ok(Ok(out)) => Ok(out),
            Ok(Err(granary_services::ai::pdf_import::PdfImportFail::Import(e))) => {
                Err(Fail::Import(e.to_string()))
            }
            Ok(Err(e)) => Err(Fail::Other(e.to_string())),
        }
    }
    .await;
    let out = match converted {
        Ok(out) => out,
        Err(Fail::Timeout) => {
            let msg = format!(
                "Conversion timed out after {} minutes. Try again, or split the document into smaller parts.",
                PDF_IMPORT_TIMEOUT_SECONDS / 60
            );
            set_import_outcome(&db, art.id, "failed", Some(&msg)).await?;
            return Ok("KB PDF import: timed out".into());
        }
        Err(Fail::Import(e)) => {
            set_import_outcome(&db, art.id, "failed", Some(&head(&e, 1000))).await?;
            return Ok(format!("KB PDF import failed: {e}"));
        }
        Err(Fail::Other(e)) => {
            let msg = format!("Unexpected error: {}", head(&e, 400));
            set_import_outcome(&db, art.id, "failed", Some(&msg)).await?;
            return Ok(format!("KB PDF import error: {e}"));
        }
    };

    let markdown = out
        .get("markdown")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mode = out
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let pages = out.get("pages_converted").cloned().unwrap_or(Value::Null);
    let truncated = out.get("truncated").and_then(Value::as_bool);
    {
        let mut conn = db.conn().await?;
        sqlx::query(
            "UPDATE kb_articles SET content = $2, pdf_import_status = 'completed', pdf_import_error = NULL, \
             pdf_import_mode = $3, pdf_import_pages = $4, pdf_import_truncated = $5, updated_at = now() \
             WHERE kb_articles.id = $1",
        )
        .bind(art.id)
        .bind(&markdown)
        .bind(&mode)
        .bind(pages.as_i64().map(|p| p as i32))
        .bind(truncated)
        .execute(&mut *conn)
        .await?;
    }
    // expire_on_commit=False: the status loaded at the start.
    let was_published = art.status.as_deref() == Some("published");
    db.commit().await?;

    // Normally the article is a draft and indexing waits for a human to publish it. If it is already
    // published (e.g. a re-convert), its embeddings are now stale.
    if was_published {
        granary_services::tasks::delay(
            "reindex_kb_embeddings",
            vec![json!(art.id.to_string())],
            serde_json::Map::new(),
        )
        .await
        .map_err(|e| Crash {
            kind: "OperationalError",
            msg: detail_or_str(&e),
        })?;
    }
    Ok(format!(
        "KB PDF import: article {article_id} -> {pages} pages ({mode})"
    ))
}

/// `_mark_crashed(reason)`: in its own session, so it still works when the failure was the session.
async fn mark_crashed(ctx: &TaskCtx, article_id: &str, reason: &str) -> Result<(), anyhow::Error> {
    let id = parse_id(article_id)?;
    let db = ctx.session();
    let Some(art) = load_article(&db, id).await.map_err(err)? else {
        return Ok(());
    };
    if matches!(
        art.pdf_import_status.as_deref(),
        Some("pending") | Some("processing")
    ) {
        set_import_outcome(&db, art.id, "failed", Some(&head(reason, 1000)))
            .await
            .map_err(|c| anyhow::anyhow!("{}", c.msg))?;
    }
    Ok(())
}

/// `sweep_stuck_pdf_imports()`: fail KB imports that have been pending/processing far longer than a
/// conversion takes (a lost job would otherwise show "converting..." forever).
pub async fn sweep_stuck_pdf_imports(ctx: TaskCtx) -> TaskResult {
    let cutoff = Utc::now() - TimeDelta::minutes(PDF_IMPORT_STALE_MINUTES);
    let db = ctx.session();
    let ids: Vec<Uuid> = {
        let mut conn = db.conn().await.map_err(err)?;
        sqlx::query_scalar(
            "UPDATE kb_articles SET pdf_import_status = 'failed', pdf_import_error = $2, updated_at = now() \
             WHERE kb_articles.pdf_import_status IN ('pending', 'processing') AND kb_articles.updated_at < $1 \
             RETURNING kb_articles.id",
        )
        .bind(cutoff)
        .bind(format!(
            "The conversion never finished (no progress for over {PDF_IMPORT_STALE_MINUTES} minutes). \
             The background worker may have been unavailable. Use Re-convert to try again."
        ))
        .fetch_all(&mut *conn)
        .await
        .map_err(err)?
    };
    if !ids.is_empty() {
        db.commit().await.map_err(err)?;
    }
    Outcome::done(format!(
        "KB PDF sweep: {} stuck import(s) marked failed",
        ids.len()
    ))
}

// ─── Approvals / signatures ──────────────────────────────────────────────────

/// `remind_overdue_approvals()`: remind approvers (email + WhatsApp) of approval steps past their SLA,
/// once a day per request.
pub async fn remind_overdue_approvals(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let count = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::approvals::engine::remind_overdue(&mut conn, None)
            .await
            .map_err(err)?
    };
    db.commit().await.map_err(err)?;
    granary_services::approvals::notify::deliver_outbox_now().await;
    Outcome::done(format!("Reminded {count} overdue approval(s)"))
}

/// `remind_expiring_signatures()`: re-invite signers of document signature requests whose deadline is
/// near (sign.reminder_* settings).
pub async fn remind_expiring_signatures(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let res = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::verification::signing_flow::remind_expiring(&mut conn).await
    };
    let count = match res {
        Ok(n) => n,
        Err(e) => match setting_detail(&e) {
            Some(detail) => {
                tracing::error!(error = %detail, "signature_reminders_not_configured");
                return Outcome::done(format!("Signature reminders skipped: {detail}"));
            }
            None => return Err(err(e)),
        },
    };
    db.commit().await.map_err(err)?;
    Outcome::done(format!("Sent {count} signature deadline reminder(s)"))
}

// ─── Quotations / mail / AI knowledge ────────────────────────────────────────

/// `expire_quotations()`: once a day, sent quotations past their validity become expired
/// (services/quotation_expiry.py).
pub async fn expire_quotations(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let today = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_core::clock::today_local(&mut conn)
            .await
            .map_err(store_err)?
    };
    let today = granary_compat::pyjson::date_isoformat(today);
    if _already_ran(&db, "sales.quotation.last_expiry_pass", &today)
        .await
        .map_err(err)?
    {
        return Outcome::done("Expiry already ran today");
    }
    let done = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::quotation_expiry::expire_due(&mut conn, None)
            .await
            .map_err(err)?
    };
    _mark_ran(
        &db,
        "sales.quotation.last_expiry_pass",
        &today,
        "sales",
        "Internal marker: last quotation expiry pass",
    )
    .await
    .map_err(err)?;
    db.commit().await.map_err(err)?;
    let mut out = format!("Expired {} quotation(s)", done.len());
    if !done.is_empty() {
        out += &format!(": {}", done[..done.len().min(20)].join(", "));
    }
    Outcome::done(out)
}

/// doclib A1 (Rust only): once a day, the owner of a library document whose review date is near
/// (documents.review_reminder_days) or past gets one reminder (not again within 7 days).
pub async fn remind_document_reviews(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let n = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::doc_library::notify::review_reminders(&mut conn)
            .await
            .map_err(err)?
    };
    db.commit().await.map_err(err)?;
    Outcome::done(format!("Reminded {n} document owner(s)"))
}

/// fix-longapply (Rust only): every 5 minutes, a Nary card still `applying` after nary.applying_stale_minutes
/// whose work is not running (no lease) becomes failed with the reason, and the Owner is told.
pub async fn nary_fail_stuck_cards(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let n = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::nary::executor::fail_stuck(&mut conn)
            .await
            .map_err(err)?
    };
    db.commit().await.map_err(err)?;
    Outcome::done(format!("Failed {n} stuck card(s)"))
}

/// 3 Nov 2026 (Rust only): every minute, the work queue's nudge to the Owner at nary.queue.nudge_times on working
/// days and at once for new urgent items (nary::work_queue). Runs while Nary is paused (that is the point).
pub async fn nary_work_queue_nudge(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::nary::work_queue::nudge_tick(&mut conn)
            .await
            .map_err(err)?
    };
    db.commit().await.map_err(err)?;
    Outcome::done(out.to_string())
}

/// `sync_personal_mailboxes()`: every few minutes, new mail in each personal mailbox's Inbox and Sent,
/// and read/unread changes made in Thunderbird or webmail.
pub async fn sync_personal_mailboxes(ctx: TaskCtx) -> TaskResult {
    let out = granary_api::routers::email_client::sync_all_accounts(ctx.state.db.clone()).await;
    Outcome::done(format!("Synced {} mailbox(es)", out["accounts"]))
}

/// `index_ai_knowledge_doc(doc_id)`: convert (PDF) and index one AI knowledge document.
pub async fn index_ai_knowledge_doc(ctx: TaskCtx) -> TaskResult {
    let doc_id = ctx.str_arg("doc_id");
    let id = granary_compat::text::utils::py_uuid(&doc_id)
        .ok_or_else(|| anyhow::anyhow!("badly formed hexadecimal UUID string"))?;
    let db = ctx.session();
    let doc: Option<AIKnowledgeDoc> = {
        let mut conn = db.conn().await.map_err(err)?;
        sqlx::query_as("SELECT * FROM ai_knowledge_docs WHERE ai_knowledge_docs.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(err)?
    };
    let Some(mut doc) = doc else {
        return Outcome::done("Document no longer exists");
    };
    let mut pdf_bytes = None;
    // A PDF is converted once; re-indexing afterwards works from the stored Markdown.
    if doc.source_type == "pdf"
        && granary_compat::text::pystr::strip(doc.content.as_deref().unwrap_or("")).is_empty()
    {
        if let Some(key) = doc.original_object_key.clone().filter(|k| !k.is_empty()) {
            let bucket = granary_core::config::settings()
                .MINIO_BUCKET_DOCUMENTS
                .clone();
            match granary_services::storage::minio()
                .get_object(&bucket, &key)
                .await
            {
                Ok(b) => pdf_bytes = Some(b),
                Err(e) => {
                    {
                        let mut conn = db.conn().await.map_err(err)?;
                        sqlx::query(
                            "UPDATE ai_knowledge_docs SET status = 'failed', error = $2, updated_at = now() \
                             WHERE ai_knowledge_docs.id = $1",
                        )
                        .bind(doc.id)
                        .bind(format!("The uploaded file could not be read back: {e}"))
                        .execute(&mut *conn)
                        .await
                        .map_err(err)?;
                    }
                    db.commit().await.map_err(err)?;
                    return Outcome::done("failed");
                }
            }
        }
    }
    {
        let mut conn = db.conn().await.map_err(err)?;
        let user_id = doc.created_by_id;
        granary_services::ai::knowledge::process(
            &mut conn,
            &mut doc,
            pdf_bytes.as_deref(),
            user_id,
        )
        .await
        .map_err(err)?;
    }
    db.commit().await.map_err(err)?;
    Outcome::done(format!(
        "{}: {} ({} chunks)",
        doc.title, doc.status, doc.chunk_count
    ))
}

/// 17 Nov 2026 (Rust only): R&D backlog items <-> GitLab issues, every repository with issue sync on
/// (services/rnd_gitlab.rs; it commits after every item, so a GitLab write is never repeated by a rollback).
pub async fn gitlab_issue_sync_tick(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::rnd_gitlab::tick(&mut conn)
            .await
            .map_err(err)?
    };
    db.commit().await.map_err(err)?;
    Outcome::done(out.to_string())
}
