//! Port of backend/app/services/approvals/notify.py.
//!
//! Approval notifications.
//!
//! In-app Notification rows are written inside the caller's transaction. Email and WhatsApp are
//! queued on the session and delivered after commit (`dispatch_outbox`) on a background task with its
//! own session, so a slow SMTP server or an offline WhatsApp service never blocks or rolls back an approval.
//!
//! The outbox: Python keeps it in `db.info["approval_outbox"]` (per session). A `&mut PgConnection` has no
//! such slot, so here it lives in a tokio task-local that [`scope`] opens: run each request (or job) inside
//! `notify::scope(async { ... })`, the Rust counterpart of "one session per request". `notify`/`push` queue
//! into the innermost open scope; `dispatch_outbox`, `deliver_outbox_now` and `discard_outbox` pop it.
//! Queuing outside any scope logs a warning and drops the email/WhatsApp part (the bell row is still written).
//! Difference: Python code that opens a second session inside a request queues into that session's own
//! `info`; here every queue call of the task shares the one scope.

use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use granary_compat::pyfmt::html_escape;
use granary_core::errors::ApiError;
use once_cell::sync::OnceCell;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// Tests switch this off so no background delivery outlives the test (Python: module attribute
/// `EXTERNAL_DELIVERY_ENABLED`, monkeypatched by the approval tests).
pub static EXTERNAL_DELIVERY_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_external_delivery_enabled(on: bool) {
    EXTERNAL_DELIVERY_ENABLED.store(on, Ordering::SeqCst);
}

pub fn external_delivery_enabled() -> bool {
    EXTERNAL_DELIVERY_ENABLED.load(Ordering::SeqCst)
}

/// One queued email/WhatsApp message (`{"user_ids", "title", "body", "link", "email", "whatsapp"}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    pub user_ids: Vec<String>,
    pub title: String,
    pub body: String,
    pub link: String,
    pub email: bool,
    pub whatsapp: bool,
}

tokio::task_local! {
    static OUTBOX: RefCell<Vec<OutboxItem>>;
}

/// Runs `fut` with its own approval outbox (the session's `info` dict).
pub async fn scope<F: Future>(fut: F) -> F::Output {
    OUTBOX.scope(RefCell::new(Vec::new()), fut).await
}

/// True inside [`scope`].
pub fn in_scope() -> bool {
    OUTBOX.try_with(|_| ()).is_ok()
}

/// The items queued so far in the current scope (for tests and diagnostics).
pub fn pending_outbox() -> Vec<OutboxItem> {
    OUTBOX.try_with(|o| o.borrow().clone()).unwrap_or_default()
}

fn queue(item: OutboxItem) {
    if OUTBOX.try_with(|o| o.borrow_mut().push(item)).is_err() {
        tracing::warn!("approval_outbox_without_scope: email/WhatsApp dropped (wrap the caller in notify::scope)");
    }
}

fn pop_all() -> Vec<OutboxItem> {
    OUTBOX
        .try_with(|o| std::mem::take(&mut *o.borrow_mut()))
        .unwrap_or_default()
}

/// `list(dict.fromkeys(str(u) for u in user_ids if u))`.
fn unique_ids<I, S>(user_ids: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: ToString,
{
    let mut out: Vec<String> = vec![];
    for u in user_ids {
        let s = u.to_string();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// The keyword arguments of `notify(...)`, with Python's defaults (`priority="normal", email=True,
/// whatsapp=False, type=None`).
#[derive(Debug, Clone)]
pub struct Notice<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub link: &'a str,
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    pub priority: &'a str,
    pub email: bool,
    pub whatsapp: bool,
    pub type_: Option<&'a str>,
}

impl<'a> Notice<'a> {
    pub fn new(
        title: &'a str,
        body: &'a str,
        link: &'a str,
        entity_type: &'a str,
        entity_id: &'a str,
    ) -> Self {
        Notice {
            title,
            body,
            link,
            entity_type,
            entity_id,
            priority: "normal",
            email: true,
            whatsapp: false,
            type_: None,
        }
    }
    pub fn priority(mut self, p: &'a str) -> Self {
        self.priority = p;
        self
    }
    pub fn email(mut self, on: bool) -> Self {
        self.email = on;
        self
    }
    pub fn whatsapp(mut self, on: bool) -> Self {
        self.whatsapp = on;
        self
    }
    pub fn type_(mut self, t: &'a str) -> Self {
        self.type_ = Some(t);
        self
    }
}

/// `title[:255]` (code points).
fn cut(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `notify(db, user_ids, *, title, body, link, entity_type, entity_id, priority, email, whatsapp, type)`:
/// a bell notification per distinct user (+ optional email/WhatsApp after commit). `type` defaults to the
/// approval types; other features pass their own (e.g. "crm_assignment"). Empty ids are skipped (Python's
/// `if u`); pass `Option`s through `.flatten()`.
pub async fn notify<I, S>(db: &mut PgConnection, user_ids: I, n: Notice<'_>) -> Result<(), ApiError>
where
    I: IntoIterator<Item = S>,
    S: ToString,
{
    let unique = unique_ids(user_ids);
    // `type or (...)`: an empty type falls back too.
    let type_ = n
        .type_
        .filter(|t| !t.is_empty())
        .unwrap_or(if n.priority != "low" {
            "approval_required"
        } else {
            "approval_update"
        });
    for uid in &unique {
        let user_id = Uuid::parse_str(uid).map_err(|e| {
            ApiError::internal(anyhow::anyhow!("invalid notification user id {uid:?}: {e}"))
        })?;
        sqlx::query(
            "INSERT INTO notifications (id, user_id, type, title, body, link, entity_type, entity_id, is_read, priority) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, $9)",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(type_)
        .bind(cut(n.title, 255))
        .bind(n.body)
        .bind(n.link)
        .bind(n.entity_type)
        .bind(n.entity_id)
        .bind(n.priority)
        .execute(&mut *db)
        .await?;
    }
    if !unique.is_empty() && (n.email || n.whatsapp) {
        queue(OutboxItem {
            user_ids: unique,
            title: n.title.to_string(),
            body: n.body.to_string(),
            link: n.link.to_string(),
            email: n.email,
            whatsapp: n.whatsapp,
        });
    }
    Ok(())
}

/// `push(db, user_ids, *, title, body, link, email=False, whatsapp=True)`: an email/WhatsApp message with no
/// bell entry (e.g. one digest for several bell entries), after commit. Python's `db` argument only reached
/// `db.info`, so it is not taken here.
pub fn push<I, S>(user_ids: I, title: &str, body: &str, link: &str, email: bool, whatsapp: bool)
where
    I: IntoIterator<Item = S>,
    S: ToString,
{
    let unique = unique_ids(user_ids);
    if !unique.is_empty() && (email || whatsapp) {
        queue(OutboxItem {
            user_ids: unique,
            title: title.to_string(),
            body: body.to_string(),
            link: link.to_string(),
            email,
            whatsapp,
        });
    }
}

/// `dispatch_outbox(db)`: call after a successful commit. Delivers on a background task.
pub fn dispatch_outbox() {
    let items = pop_all();
    if items.is_empty() || !external_delivery_enabled() {
        return;
    }
    tokio::spawn(deliver(items));
}

/// `deliver_outbox_now(db)`: for callers whose runtime ends when they return (scheduled jobs): deliver
/// before returning.
pub async fn deliver_outbox_now() {
    let items = pop_all();
    if !items.is_empty() && external_delivery_enabled() {
        deliver(items).await;
    }
}

/// `discard_outbox(db)`.
pub fn discard_outbox() {
    let _ = pop_all();
}

/// The source half of `move_outbox(source, target)`: takes what the current (inner) scope queued.
pub fn take_outbox() -> Vec<OutboxItem> {
    pop_all()
}

/// `move_outbox(source, target)`: hand what a nested scope (`source`, taken with [`take_outbox`]) queued and
/// committed to the current scope (`target`), whose caller delivers its outbox after its own commit.
pub fn move_outbox(items: Vec<OutboxItem>) {
    if items.is_empty() {
        return;
    }
    for item in items {
        queue(item);
    }
}

static DELIVERY_POOL: OnceCell<PgPool> = OnceCell::new();

/// Installs the pool `_deliver` opens its own session from (Python: `AsyncSessionLocal`). Without it a
/// lazy pool on `DATABASE_URL` is used.
pub fn set_delivery_pool(pool: PgPool) -> bool {
    DELIVERY_POOL.set(pool).is_ok()
}

fn delivery_pool() -> Result<&'static PgPool, ApiError> {
    DELIVERY_POOL.get_or_try_init(|| {
        let url = granary_core::state::normalize_database_url(
            &granary_core::config::settings().DATABASE_URL,
        );
        PgPool::connect_lazy(&url).map_err(ApiError::from)
    })
}

/// `_deliver(items)`: never raises (each failure is logged, like Python's per-message `except`); a failure
/// outside the per-message guards ends the delivery like Python's unhandled task exception.
async fn deliver(items: Vec<OutboxItem>) {
    if let Err(e) = deliver_inner(items).await {
        tracing::error!(error = %e, "approval_outbox_delivery_failed");
    }
}

async fn deliver_inner(items: Vec<OutboxItem>) -> Result<(), ApiError> {
    let pool = delivery_pool()?;
    let mut db = pool.acquire().await?;
    let settings = granary_core::config::settings();
    for item in items {
        let ids: Vec<Uuid> = item
            .user_ids
            .iter()
            .filter_map(|u| Uuid::parse_str(u).ok())
            .collect();
        let rows: Vec<(Uuid, String, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT users.id, users.email, users.name, employees.whatsapp, employees.phone \
             FROM users LEFT OUTER JOIN employees ON employees.user_id = users.id \
             WHERE users.id = ANY($1) AND users.is_active = true",
        )
        .bind(&ids)
        .fetch_all(&mut *db)
        .await?;
        let url = format!("{}{}", settings.APP_URL, item.link);
        for (user_id, email, name, whatsapp, phone) in rows {
            if item.email && !email.is_empty() {
                let html = email_html(Some(&name), &item.title, &item.body, &url);
                if let Err(e) = send_email(&mut db, &email, &item.title, &html).await {
                    tracing::warn!(user = %user_id, error = %e, "approval_email_failed");
                }
            }
            if item.whatsapp {
                // A chat the person linked on their profile wins over the Employee number.
                let mut to = crate::nary::identity::preferred_chat(&mut db, user_id)
                    .await?
                    .filter(|s| !s.is_empty());
                if to.is_none() {
                    let number = whatsapp
                        .filter(|s| !s.is_empty())
                        .or(phone.filter(|s| !s.is_empty()));
                    if let Some(n) = number {
                        to = Some(granary_compat::text::utils::normalize_phone(Some(&n)))
                            .filter(|s| !s.is_empty());
                    }
                }
                let Some(to) = to else { continue };
                let text = format!("*{}*\n{}\n{}", item.title, item.body, url);
                if let Err(e) = crate::whatsapp::send_message(&to, &text).await {
                    tracing::warn!(user = %user_id, error = %e, "approval_whatsapp_failed");
                }
            }
        }
    }
    Ok(())
}

/// `send_email(to=..., subject=..., db=db, html_body=...)`.
async fn send_email(
    db: &mut PgConnection,
    to: &str,
    subject: &str,
    html: &str,
) -> Result<(), String> {
    crate::email::send_email(&[to.to_string()], subject, html, db, None, None)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `_email_html(name, title, body, url)`, byte for byte.
pub fn email_html(name: Option<&str>, title: &str, body: &str, url: &str) -> String {
    let settings = granary_core::config::settings();
    let first = settings.APP_NAME.split_whitespace().next().unwrap_or("");
    let e = |s: &str| html_escape(s, true);
    format!(
        "\n    <html><body style=\"font-family:Arial,sans-serif;color:#111;max-width:560px;margin:40px auto;padding:0 20px\">\n      \
<div style=\"border-bottom:3px solid #1f4e79;padding-bottom:16px;margin-bottom:24px\">\n        \
<span style=\"font-size:22px;font-weight:900;color:#1f4e79;letter-spacing:3px\">{}</span>\n      \
</div>\n      \
<p>Hello <strong>{}</strong>,</p>\n      \
<h2 style=\"font-size:18px;color:#111\">{}</h2>\n      \
<p style=\"color:#444;white-space:pre-line\">{}</p>\n      \
<div style=\"margin:28px 0\">\n        \
<a href=\"{}\" style=\"background:#1f4e79;color:#fff;padding:12px 26px;border-radius:10px;\n           \
font-weight:700;text-decoration:none;display:inline-block\">Open in {}</a>\n      \
</div>\n      \
<p style=\"font-size:12px;color:#888\">Approvals are recorded in the ERP: sign in to approve or reject.</p>\n    \
</body></html>\n    ",
        e(first),
        e(name.unwrap_or("")),
        e(title),
        e(body),
        e(url),
        e(&settings.APP_NAME),
    )
}
