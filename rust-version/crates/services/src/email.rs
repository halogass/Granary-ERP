//! Port of backend/app/services/email.py.
//!
//! SMTP email service. All SMTP configuration is stored in SystemSetting (category 'email' and
//! 'crm_email'), encrypted at rest with Fernet. No SMTP credentials live in .env.
//!
//! The message is built and serialised exactly as Python's `email` package (compat32) does it
//! (compat32.rs: same headers, multipart nesting, base64 bodies, encoded-word headers, folding, boundary
//! and Message-ID shapes), and handed to the SMTP server raw through `lettre` (STARTTLS when `tls`, login
//! when a user is set, envelope from the From/To/Cc headers as aiosmtplib picks it).
//!
//! Seams: Python tests monkeypatch `_send_via_config`, `load_system_smtp` and `load_crm_smtp`. Here
//! `Seams` replaces them, process-wide (`set_global_seams`) or for one task (`with_seams`). Every caller
//! goes through the public functions, so an override applies to them all, as a monkeypatch does.

pub mod compat32;

use std::future::Future;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use once_cell::sync::Lazy;
use sqlx::PgConnection;

pub use crate::email_crypto::{decrypt, encrypt};
use compat32::Message;

// ─── Keys ────────────────────────────────────────────────────────────────────

const _SYSTEM_SMTP_KEYS: &[&str] = &[
    "email_smtp_host",
    "email_smtp_port",
    "email_smtp_user",
    "email_smtp_password",
    "email_smtp_from_name",
    "email_smtp_from_email",
    "email_smtp_tls",
];

const _CRM_SMTP_KEYS: &[&str] = &[
    "crm_smtp_host",
    "crm_smtp_port",
    "crm_smtp_user",
    "crm_smtp_password",
    "crm_smtp_from_name",
    "crm_smtp_from_email",
    "crm_smtp_tls",
];

const _CRM_IMAP_KEYS: &[&str] = &[
    "crm_imap_host",
    "crm_imap_port",
    "crm_imap_user",
    "crm_imap_password",
    "crm_imap_ssl",
];

pub const ENCRYPTED_EMAIL_KEYS: &[&str] = &[
    "email_smtp_password",
    "crm_smtp_password",
    "crm_imap_password",
];

// ─── Types ───────────────────────────────────────────────────────────────────

/// The SMTP config dict (`{"host", "port", "user", "password", "from_name", "from_email", "tls"}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: i64,
    pub user: String,
    pub password: String,
    pub from_name: String,
    pub from_email: String,
    pub tls: bool,
}

/// The IMAP config dict (`{"host", "port", "user", "password", "ssl"}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImapConfig {
    pub host: String,
    pub port: i64,
    pub user: String,
    pub password: String,
    pub ssl: bool,
}

/// One attachment dict: `{"filename", "content", "mime_type"}` (Python ignores `mime_type`: every
/// attachment goes out as application/octet-stream).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub filename: String,
    pub content: Vec<u8>,
    pub mime_type: Option<String>,
}

/// The `capture` dict: the exact bytes (as `Message.as_bytes()`) and the Message-ID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capture {
    pub raw: Option<Vec<u8>>,
    pub message_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    /// What aiosmtplib raises (connection, TLS, auth, refused recipients, ...).
    #[error("{0}")]
    Smtp(String),
    /// `int(...)` of a port setting failed (ValueError).
    #[error("{0}")]
    Value(String),
    /// `decrypt` failed (cryptography.fernet.InvalidToken).
    #[error("InvalidToken")]
    Decrypt(crate::email_crypto::FernetError),
    #[error(transparent)]
    HeaderParse(#[from] compat32::HeaderParseError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Other(String),
}

// ─── Seams (monkeypatch points) ──────────────────────────────────────────────

/// The arguments `_send_via_config` was called with (what a Python fake receives).
#[derive(Debug, Clone)]
pub struct SendCall {
    pub cfg: SmtpConfig,
    pub to: Vec<String>,
    /// Python's `to` was a str (not a list).
    pub to_is_str: bool,
    pub subject: String,
    pub html_body: String,
    pub cc: Option<Vec<String>>,
    pub attachments: Option<Vec<Attachment>>,
    pub has_db: bool,
    pub headers: Option<Vec<(String, String)>>,
    pub wants_capture: bool,
    /// R&D 26 Oct: blind copies (envelope only, never a header)
    pub bcc: Vec<String>,
}

/// A replacement `_send_via_config`: returns the bool and, when the caller asked for a capture, what to
/// put in it.
pub type SendHook = Arc<
    dyn Fn(SendCall) -> BoxFuture<'static, Result<(bool, Option<Capture>), EmailError>>
        + Send
        + Sync,
>;
/// A replacement `load_system_smtp` / `load_crm_smtp`.
pub type SmtpHook =
    Arc<dyn Fn() -> BoxFuture<'static, Result<SmtpConfig, EmailError>> + Send + Sync>;

#[derive(Clone, Default)]
pub struct Seams {
    pub send_via_config: Option<SendHook>,
    pub load_system_smtp: Option<SmtpHook>,
    pub load_crm_smtp: Option<SmtpHook>,
}

static GLOBAL_SEAMS: Lazy<RwLock<Option<Seams>>> = Lazy::new(|| RwLock::new(None));
tokio::task_local! {
    static SCOPED_SEAMS: Seams;
}

/// Install (or with None remove) process-wide overrides.
pub fn set_global_seams(seams: Option<Seams>) {
    *GLOBAL_SEAMS.write().unwrap() = seams;
}

/// Run `f` with `seams` (task-local; spawned tasks do not inherit them).
pub async fn with_seams<F: Future>(seams: Seams, f: F) -> F::Output {
    SCOPED_SEAMS.scope(seams, f).await
}

// fix-longapply: what a detached task carries over (nary::long_work).
/// The seams of the enclosing [`with_seams`], if any.
pub fn scoped_seams() -> Option<Seams> {
    SCOPED_SEAMS.try_with(|s| s.clone()).ok()
}

fn seams() -> Option<Seams> {
    if let Ok(s) = SCOPED_SEAMS.try_with(|s| s.clone()) {
        return Some(s);
    }
    GLOBAL_SEAMS.read().unwrap().clone()
}

// ─── Config loaders ──────────────────────────────────────────────────────────

async fn _load_settings(
    db: &mut PgConnection,
    keys: &[&str],
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE key = ANY($1)")
            .bind(&keys)
            .fetch_all(&mut *db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(k, v)| (k, v.unwrap_or_default()))
        .collect())
}

fn get<'a>(m: &'a [(String, String)], k: &str) -> Option<&'a str> {
    m.iter()
        .rev()
        .find(|(key, _)| key == k)
        .map(|(_, v)| v.as_str())
}

/// `int(m.get(key) or default)`.
fn port_of(m: &[(String, String)], key: &str, default: i64) -> Result<i64, EmailError> {
    match get(m, key) {
        Some(v) if !v.is_empty() => granary_compat::crypto::mcp::py_int_str(v).ok_or_else(|| {
            EmailError::Value(format!(
                "invalid literal for int() with base 10: {}",
                py_repr(v)
            ))
        }),
        _ => Ok(default),
    }
}

fn py_repr(s: &str) -> String {
    granary_compat::pyjson::repr(&granary_compat::pyjson::PyValue::Str(s.to_string()))
        .unwrap_or_default()
}

fn password_of(m: &[(String, String)], key: &str) -> Result<String, EmailError> {
    match get(m, key) {
        Some(v) if !v.is_empty() => decrypt(v).map_err(EmailError::Decrypt),
        _ => Ok(String::new()),
    }
}

/// `m.get(key, "true").lower() == "true"`.
fn flag_of(m: &[(String, String)], key: &str) -> bool {
    granary_compat::text::pystr::lower(get(m, key).unwrap_or("true")) == "true"
}

/// R&D 26 Oct: an extra CRM inbox's SSL/TLS switch: empty (never saved) is on, as Settings shows it.
fn flag_on_unless_off(m: &[(String, String)], key: &str) -> bool {
    match get(m, key).map(|v| v.trim().to_lowercase()) {
        Some(v) if !v.is_empty() => v == "true",
        _ => true,
    }
}

async fn smtp_from(
    db: &mut PgConnection,
    keys: &[&str],
    prefix: &str,
    default_from: &str,
) -> Result<SmtpConfig, EmailError> {
    let m = _load_settings(db, keys).await?;
    let k = |s: &str| format!("{prefix}_{s}");
    Ok(SmtpConfig {
        host: get(&m, &k("host")).unwrap_or("").to_string(),
        port: port_of(&m, &k("port"), 587)?,
        user: get(&m, &k("user")).unwrap_or("").to_string(),
        password: password_of(&m, &k("password"))?,
        from_name: get(&m, &k("from_name")).unwrap_or(default_from).to_string(),
        from_email: get(&m, &k("from_email")).unwrap_or("").to_string(),
        tls: flag_of(&m, &k("tls")),
    })
}

/// `load_system_smtp(db)`.
pub async fn load_system_smtp(db: &mut PgConnection) -> Result<SmtpConfig, EmailError> {
    if let Some(h) = seams().and_then(|s| s.load_system_smtp) {
        return h().await;
    }
    smtp_from(db, _SYSTEM_SMTP_KEYS, "email_smtp", "Granary").await
}

/// `load_crm_smtp(db)`.
pub async fn load_crm_smtp(db: &mut PgConnection) -> Result<SmtpConfig, EmailError> {
    if let Some(h) = seams().and_then(|s| s.load_crm_smtp) {
        return h().await;
    }
    smtp_from(db, _CRM_SMTP_KEYS, "crm_smtp", "Your Company").await
}

/// R&D 26 Oct (several CRM inboxes): the SMTP settings of the inbox whose keys start with `prefix`
/// ("crm" is hello@, the first inbox: [`load_crm_smtp`]; another inbox has its own, e.g. "crm_sales").
pub async fn load_smtp_prefixed(
    db: &mut PgConnection,
    prefix: &str,
) -> Result<SmtpConfig, EmailError> {
    if prefix == "crm" {
        return load_crm_smtp(db).await;
    }
    let keys: Vec<String> = mailbox_keys(prefix)
        .into_iter()
        .filter(|k| k.contains("_smtp_"))
        .collect();
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    let mut cfg = smtp_from(db, &refs, &format!("{prefix}_smtp"), "Your Company").await?;
    let m = _load_settings(db, &[format!("{prefix}_smtp_tls").as_str()]).await?;
    cfg.tls = flag_on_unless_off(&m, &format!("{prefix}_smtp_tls"));
    Ok(cfg)
}

/// The IMAP settings of the inbox whose keys start with `prefix` (see [`load_smtp_prefixed`]).
pub async fn load_imap_prefixed(
    db: &mut PgConnection,
    prefix: &str,
) -> Result<ImapConfig, EmailError> {
    if prefix == "crm" {
        return load_crm_imap(db).await;
    }
    let keys = mailbox_keys(prefix);
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    let m = _load_settings(db, &refs).await?;
    let k = |s: &str| format!("{prefix}_imap_{s}");
    Ok(ImapConfig {
        host: get(&m, &k("host")).unwrap_or("").to_string(),
        port: port_of(&m, &k("port"), 993)?,
        user: get(&m, &k("user")).unwrap_or("").to_string(),
        password: password_of(&m, &k("password"))?,
        ssl: flag_on_unless_off(&m, &k("ssl")),
    })
}

/// Every settings key of an inbox's mail servers (the same names as hello@'s crm_* keys).
pub fn mailbox_keys(prefix: &str) -> Vec<String> {
    _CRM_SMTP_KEYS
        .iter()
        .chain(_CRM_IMAP_KEYS.iter())
        .map(|k| format!("{prefix}{}", &k["crm".len()..]))
        .collect()
}

/// Whether a settings key holds a mail server password (stored encrypted).
pub fn is_mail_password_key(key: &str) -> bool {
    ENCRYPTED_EMAIL_KEYS.contains(&key)
        || (key.starts_with("crm_")
            && (key.ends_with("_smtp_password") || key.ends_with("_imap_password")))
}

/// `load_crm_imap(db)`.
pub async fn load_crm_imap(db: &mut PgConnection) -> Result<ImapConfig, EmailError> {
    let m = _load_settings(db, _CRM_IMAP_KEYS).await?;
    Ok(ImapConfig {
        host: get(&m, "crm_imap_host").unwrap_or("").to_string(),
        port: port_of(&m, "crm_imap_port", 993)?,
        user: get(&m, "crm_imap_user").unwrap_or("").to_string(),
        password: password_of(&m, "crm_imap_password")?,
        ssl: flag_of(&m, "crm_imap_ssl"),
    })
}

// ─── Core send ───────────────────────────────────────────────────────────────

/// One image, tagged with the Content-ID the HTML refers to.
fn _inline_image_part(part: &crate::email_signature::InlinePart) -> Message {
    let mime = if part.mime.is_empty() {
        "image/png"
    } else {
        part.mime.as_str()
    };
    let subtype = mime.split_once('/').map(|x| x.1).unwrap_or(mime);
    let mut img = Message::image(&part.data, subtype);
    img.add_header("Content-ID", &format!("<{}>", part.cid), &[]);
    let filename = if part.filename.is_empty() {
        "image"
    } else {
        part.filename.as_str()
    };
    img.add_header("Content-Disposition", "inline", &[("filename", filename)]);
    img
}

/// The message `_send_via_config` builds, before serialisation. `date` is `formatdate(localtime=True)`;
/// `make_msgid` is called only when `headers` carries no Message-ID.
#[allow(clippy::too_many_arguments)]
pub fn compose(
    cfg: &SmtpConfig,
    recipients: &[String],
    subject: &str,
    html_body: &str,
    cc: Option<&[String]>,
    attachments: Option<&[Attachment]>,
    inline_parts: &[crate::email_signature::InlinePart],
    headers: Option<&[(String, String)]>,
    date: &str,
    make_msgid: &mut dyn FnMut(Option<&str>) -> String,
) -> Message {
    let mut msg = Message::multipart("alternative");
    msg.attach(Message::text_utf8(html_body, "html"));

    let mut final_msg = msg;
    if !inline_parts.is_empty() {
        // multipart/related binds the HTML to the images it references by Content-ID.
        let mut related = Message::multipart("related");
        related.attach(final_msg);
        for part in inline_parts {
            related.attach(_inline_image_part(part));
        }
        final_msg = related;
    }
    if let Some(atts) = attachments.filter(|a| !a.is_empty()) {
        let mut mixed = Message::multipart("mixed");
        mixed.attach(final_msg);
        for att in atts {
            let mut part = Message::application(&att.content, &att.filename);
            part.set(
                "Content-Disposition",
                &format!("attachment; filename=\"{}\"", att.filename),
            );
            mixed.attach(part);
        }
        final_msg = mixed;
    }
    // Envelope headers go on the outermost part: SMTP clients read Subject/From/To (and the recipients)
    // from there, so on a wrapped message (inline images, attachments) they must not sit on the inner part.
    final_msg.set("Subject", subject);
    final_msg.set("From", &format!("{} <{}>", cfg.from_name, cfg.from_email));
    final_msg.set("To", &recipients.join(", "));
    if let Some(cc) = cc.filter(|c| !c.is_empty()) {
        final_msg.set("Cc", &cc.join(", "));
    }
    final_msg.set("Date", date);
    let mut extra: Vec<(String, String)> = headers.map(|h| h.to_vec()).unwrap_or_default();
    // extra.pop("Message-ID", None): the last value of that exact key, every copy removed (a dict key)
    let given = extra
        .iter()
        .rev()
        .find(|(k, _)| k == "Message-ID")
        .map(|(_, v)| v.clone());
    extra.retain(|(k, _)| k != "Message-ID");
    let message_id = match given.filter(|v| !v.is_empty()) {
        Some(v) => v,
        None => {
            let d = cfg.from_email.rsplit('@').next().unwrap_or("");
            make_msgid(if d.is_empty() { None } else { Some(d) })
        }
    };
    final_msg.set("Message-ID", &message_id);
    // e.g. List-Unsubscribe on marketing email, In-Reply-To on a reply (a dict: one value per key)
    let mut seen: Vec<String> = Vec::new();
    for (name, _) in &extra {
        if seen.contains(name) {
            continue;
        }
        seen.push(name.clone());
        let value = extra
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        final_msg.set(name, &value);
    }
    final_msg
}

/// What `aiosmtplib.send` puts on the wire for `msg`: the From address and the To/Cc addresses.
pub fn envelope_of(msg: &Message) -> (String, Vec<String>) {
    let from = msg
        .get("Sender")
        .or_else(|| msg.get("From"))
        .map(|v| {
            compat32::getaddresses(v)
                .into_iter()
                .next()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let mut rcpt = Vec::new();
    for name in ["To", "Cc", "Bcc"] {
        for (k, v) in &msg.headers {
            if k.eq_ignore_ascii_case(name) {
                rcpt.extend(compat32::getaddresses(v));
            }
        }
    }
    (from, rcpt)
}

/// Deliver raw bytes over SMTP as `aiosmtplib.send(..., start_tls=cfg.tls)` does. Public for
/// routers/email_client.py `send_email` (a personal account sends its own message).
pub async fn smtp_send(
    cfg: &SmtpConfig,
    from: &str,
    rcpt: &[String],
    raw: &[u8],
) -> Result<(), EmailError> {
    use lettre::address::Envelope;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::client::{Tls, TlsParameters};
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    let smtp = |e: &dyn std::fmt::Display| EmailError::Smtp(e.to_string());
    let port = u16::try_from(cfg.port)
        .map_err(|_| EmailError::Smtp(format!("port must be 0-65535, got {}", cfg.port)))?;
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(cfg.host.as_str())
        .port(port)
        .timeout(Some(std::time::Duration::from_secs(60)));
    if cfg.tls {
        let params = TlsParameters::new(cfg.host.clone()).map_err(|e| smtp(&e))?;
        b = b.tls(Tls::Required(params));
    } else {
        b = b.tls(Tls::None);
    }
    if !cfg.user.is_empty() {
        b = b.credentials(Credentials::new(cfg.user.clone(), cfg.password.clone()));
    }
    let transport = b.build();
    let from_addr = if from.is_empty() {
        None
    } else {
        Some(from.parse().map_err(|e| smtp(&e))?)
    };
    let to: Vec<lettre::Address> = rcpt
        .iter()
        .map(|r| r.parse().map_err(|e| smtp(&e)))
        .collect::<Result<_, _>>()?;
    let envelope = Envelope::new(from_addr, to).map_err(|e| smtp(&e))?;
    transport
        .send_raw(&envelope, raw)
        .await
        .map_err(|e| smtp(&e))?;
    Ok(())
}

/// Send one email. `headers` may set Message-ID / In-Reply-To / References (a reply must carry them to
/// thread in the recipient's client). With `capture`, the exact bytes and the Message-ID are handed
/// back, for the copy saved to the Sent folder.
#[allow(clippy::too_many_arguments)]
pub async fn _send_via_config(
    cfg: &SmtpConfig,
    to: &[String],
    subject: &str,
    html_body: &str,
    cc: Option<&[String]>,
    attachments: Option<&[Attachment]>,
    db: Option<&mut PgConnection>,
    headers: Option<&[(String, String)]>,
    capture: Option<&mut Capture>,
) -> Result<bool, EmailError> {
    _send_via_config_bcc(
        cfg,
        to,
        subject,
        html_body,
        cc,
        &[],
        attachments,
        db,
        headers,
        capture,
    )
    .await
}

/// [`_send_via_config`] with blind copies (R&D 26 Oct): `bcc` get the email (SMTP envelope) but appear in no
/// header, so neither the other recipients nor the Sent copy show them.
#[allow(clippy::too_many_arguments)]
pub async fn _send_via_config_bcc(
    cfg: &SmtpConfig,
    to: &[String],
    subject: &str,
    html_body: &str,
    cc: Option<&[String]>,
    bcc: &[String],
    attachments: Option<&[Attachment]>,
    db: Option<&mut PgConnection>,
    headers: Option<&[(String, String)]>,
    capture: Option<&mut Capture>,
) -> Result<bool, EmailError> {
    if let Some(h) = seams().and_then(|s| s.send_via_config) {
        let call = SendCall {
            cfg: cfg.clone(),
            to: to.to_vec(),
            to_is_str: false,
            subject: subject.to_string(),
            html_body: html_body.to_string(),
            cc: cc.map(|c| c.to_vec()),
            attachments: attachments.map(|a| a.to_vec()),
            has_db: db.is_some(),
            headers: headers.map(|h| h.to_vec()),
            wants_capture: capture.is_some(),
            bcc: bcc.to_vec(),
        };
        let (ok, cap) = h(call).await?;
        if let (Some(c), Some(got)) = (capture, cap) {
            if got.raw.is_some() {
                c.raw = got.raw;
            }
            if got.message_id.is_some() {
                c.message_id = got.message_id;
            }
        }
        return Ok(ok);
    }
    send_real(
        cfg,
        to,
        subject,
        html_body,
        cc,
        bcc,
        attachments,
        db,
        headers,
        capture,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_real(
    cfg: &SmtpConfig,
    to: &[String],
    subject: &str,
    html_body: &str,
    cc: Option<&[String]>,
    bcc: &[String],
    attachments: Option<&[Attachment]>,
    db: Option<&mut PgConnection>,
    headers: Option<&[(String, String)]>,
    capture: Option<&mut Capture>,
) -> Result<bool, EmailError> {
    if cfg.host.is_empty() || cfg.from_email.is_empty() {
        tracing::warn!(subject, "Email not configured: skipping send");
        return Ok(false);
    }
    let recipients = to;

    // Signature images travel with the message (cid:) instead of being fetched, so they render even in
    // clients that block remote content. (Python catches any exception here; the Rust function does not
    // fail, it skips what it cannot load.)
    let mut html = html_body.to_string();
    let mut inline_parts = Vec::new();
    if let Some(db) = db {
        let (h, parts) = crate::email_signature::inline_cid_images(db, &html).await;
        html = h;
        inline_parts = parts;
    }

    let date = compat32::formatdate_local(chrono::Local::now());
    let mut msg = compose(
        cfg,
        recipients,
        subject,
        &html,
        cc,
        attachments,
        &inline_parts,
        headers,
        &date,
        &mut |d| compat32::make_msgid(d),
    );
    let (from, mut rcpt) = envelope_of(&msg);
    for b in bcc {
        let b = b.trim();
        if !b.is_empty() && !rcpt.iter().any(|r| r.eq_ignore_ascii_case(b)) {
            rcpt.push(b.to_string());
        }
    }
    let result = async {
        let mut boundary = compat32::make_boundary;
        let wire = compat32::flatten(&mut msg, "\r\n", &mut boundary, false)?;
        // lettre always ends DATA with "\r\n.\r\n"; aiosmtplib adds only ".\r\n" after a message that
        // already ends in CRLF. Dropping the final CRLF here makes the received bytes identical.
        let body = wire.strip_suffix("\r\n").unwrap_or(&wire);
        smtp_send(cfg, &from, &rcpt, body.as_bytes()).await
    }
    .await;
    match result {
        Ok(()) => {
            tracing::info!(to = ?recipients, subject, "Email sent");
            if let Some(c) = capture {
                let mut boundary = compat32::make_boundary;
                c.raw = Some(compat32::flatten(&mut msg, "\n", &mut boundary, true)?.into_bytes());
                c.message_id = msg.get("Message-ID").map(|s| s.to_string());
            }
            Ok(true)
        }
        Err(e) => {
            tracing::error!(error = %e, to = ?recipients, subject, "Email send failed");
            Err(e)
        }
    }
}

/// 3 Nov 2026 (integration mail): one email whose content another service rendered, sent as it is: a text part,
/// and with `html` a multipart/alternative of text then HTML. `Reply-To` when given. Nothing of the content is
/// logged (password-reset links travel in it). The `send_via_config` seam replaces the delivery as for every
/// other ERP email (the HTML, else the text, as `html_body`; Reply-To among the headers).
#[allow(clippy::too_many_arguments)]
pub async fn send_rendered(
    cfg: &SmtpConfig,
    to: &[String],
    cc: &[String],
    reply_to: Option<&str>,
    subject: &str,
    text: &str,
    html: Option<&str>,
    extra_headers: &[(String, String)],
) -> Result<(), EmailError> {
    let mut headers: Vec<(String, String)> = extra_headers.to_vec();
    if let Some(r) = reply_to.filter(|r| !r.is_empty()) {
        headers.push(("Reply-To".to_string(), r.to_string()));
    }
    if let Some(h) = seams().and_then(|s| s.send_via_config) {
        let call = SendCall {
            cfg: cfg.clone(),
            to: to.to_vec(),
            to_is_str: false,
            subject: subject.to_string(),
            html_body: html.unwrap_or(text).to_string(),
            cc: (!cc.is_empty()).then(|| cc.to_vec()),
            attachments: None,
            has_db: false,
            headers: Some(headers),
            wants_capture: false,
            bcc: Vec::new(),
        };
        let (ok, _) = h(call).await?;
        return if ok {
            Ok(())
        } else {
            Err(EmailError::Other(
                "the mail transport refused the email".into(),
            ))
        };
    }
    if cfg.host.is_empty() || cfg.from_email.is_empty() {
        return Err(EmailError::Other(
            "System email (SMTP) is not set up: Settings → Email & WhatsApp → System email".into(),
        ));
    }
    let mut msg = match html {
        Some(h) => {
            let mut alt = Message::multipart("alternative");
            alt.attach(Message::text_utf8(text, "plain"));
            alt.attach(Message::text_utf8(h, "html"));
            alt
        }
        None => Message::text_utf8(text, "plain"),
    };
    msg.set("Subject", subject);
    msg.set("From", &format!("{} <{}>", cfg.from_name, cfg.from_email));
    msg.set("To", &to.join(", "));
    if !cc.is_empty() {
        msg.set("Cc", &cc.join(", "));
    }
    msg.set("Date", &compat32::formatdate_local(chrono::Local::now()));
    let domain = cfg.from_email.rsplit('@').next().unwrap_or("");
    msg.set(
        "Message-ID",
        &compat32::make_msgid((!domain.is_empty()).then_some(domain)),
    );
    for (k, v) in &headers {
        msg.set(k, v);
    }
    let (from, rcpt) = envelope_of(&msg);
    let mut boundary = compat32::make_boundary;
    let wire = compat32::flatten(&mut msg, "\r\n", &mut boundary, false)?;
    let body = wire.strip_suffix("\r\n").unwrap_or(&wire);
    smtp_send(cfg, &from, &rcpt, body.as_bytes()).await
}

/// `send_email(to, subject, html_body, *, db, cc=None, attachments=None)`.
pub async fn send_email(
    to: &[String],
    subject: &str,
    html_body: &str,
    db: &mut PgConnection,
    cc: Option<&[String]>,
    attachments: Option<&[Attachment]>,
) -> Result<bool, EmailError> {
    let cfg = load_system_smtp(db).await?;
    _send_via_config(
        &cfg,
        to,
        subject,
        html_body,
        cc,
        attachments,
        None,
        None,
        None,
    )
    .await
}

// ─── Test send (uses raw config dict, does NOT need a saved DB record) ──────

/// Send a test email using the provided raw SMTP config. Fails on failure.
pub async fn send_test_email(cfg: &SmtpConfig, to: &str) -> Result<(), EmailError> {
    let html = crate::email_templates::test_email_template(&cfg.from_name);
    _send_via_config(
        cfg,
        &[to.to_string()],
        "Granary: SMTP test",
        &html,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    Ok(())
}

// ─── Document emails (system transactional) ──────────────────────────────────

/// `send_quotation_email(...)`.
#[allow(clippy::too_many_arguments)]
pub async fn send_quotation_email(
    to: &str,
    contact_name: &str,
    quotation_number: &str,
    pdf_bytes: &[u8],
    db: &mut PgConnection,
    company_name: &str,
    company_email: &str,
    cc: Option<&[String]>,
) -> Result<bool, EmailError> {
    let html = crate::email_templates::quotation_email_template(
        contact_name,
        quotation_number,
        company_name,
        company_email,
        crate::email_templates::DEFAULT_COMPANY_WEBSITE,
        "",
        "",
    );
    send_email(
        &[to.to_string()],
        &format!("Quotation {quotation_number} - {company_name}"),
        &html,
        db,
        cc,
        Some(&[Attachment {
            filename: format!("{quotation_number}.pdf"),
            content: pdf_bytes.to_vec(),
            mime_type: Some("application/pdf".into()),
        }]),
    )
    .await
}

/// `send_invoice_email(...)`.
#[allow(clippy::too_many_arguments)]
pub async fn send_invoice_email(
    to: &str,
    contact_name: &str,
    invoice_number: &str,
    due_date: &str,
    pdf_bytes: &[u8],
    db: &mut PgConnection,
    company_name: &str,
    company_email: &str,
    cc: Option<&[String]>,
) -> Result<bool, EmailError> {
    let html = crate::email_templates::invoice_email_template(
        contact_name,
        invoice_number,
        due_date,
        company_name,
        company_email,
        crate::email_templates::DEFAULT_COMPANY_WEBSITE,
        "",
        "",
    );
    send_email(
        &[to.to_string()],
        &format!("Invoice {invoice_number} - {company_name}"),
        &html,
        db,
        cc,
        Some(&[Attachment {
            filename: format!("{invoice_number}.pdf"),
            content: pdf_bytes.to_vec(),
            mime_type: Some("application/pdf".into()),
        }]),
    )
    .await
}
