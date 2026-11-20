//! Nary batch 4 (D2, Rust only): files on the messages Nary sends to customers.
//!
//! ```text
//!     propose_customer_message(attach=[...])   for both channels. An item is {type, id}:
//!         quotation | invoice | sales_document  the QR-verified PDF, exactly as the ERP's Send makes it
//!                                               (delivery::nary_documents): the sender's send permission,
//!                                               the approval gate (an unapproved one goes for approval and
//!                                               the message waits), then marked sent (nary_mark_sent).
//!         file                                  any file id Nary was given: a file sent in the chat, a CRM
//!                                               attachment (AttachmentPanel: organizations, deals, product
//!                                               files, Knowledge Base files...), an email attachment of the
//!                                               CRM inbox, or a customer-safe AI knowledge document.
//!     card_items()      what the confirm card lists: name, size, type, approved or not; card_files() the files
//!                       with their thumbnails for the ERP card (29 Oct 2026).
//!     prepare()         at the send: the bytes of every item (documents through the approval gate), within
//!                       nary.attach.max_files and nary.attach.max_total_mb.
//!     send_whatsapp()   after the text: each file through /send-media, a document with its number and verify
//!                       link as the caption; a picture (png, jpeg, webp, gif) goes as a photo (29 Oct 2026),
//!                       anything else as a document; each is stored in the chat as Nary's.
//!     email             channel_actions::send_email attaches them (as_email).
//! ```
//! 1 Nov 2026 (Owner decisions), videos: on WhatsApp an MP4 up to `nary.wa.video_max_mb` (16) goes as a playable
//! video, a larger one (or a MOV / WebM) as a document (the sidecar's asDocument), up to the upload cap
//! `nary.upload.video_max_mb`; videos do not count toward `nary.attach.max_total_mb` there (each goes on its
//! own). `nary.attach.max_total_mb` is the EMAIL limit: a stored file that does not fit goes in the email as a
//! signed download link (storage::browser_link, `nary.email.link_days`), one short line per file. The card says
//! how each file goes (send_as) and shows a video's length.
//! A file is only sent when the person the message goes out for may open it (the same rule as the
//! attachment's own download), so a confirm card never carries a file its approver could not read.

use std::time::Duration;

use chrono::Utc;
use granary_core::auth::AuthUser;
use granary_core::config::settings;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::adapters::get_adapter;
use crate::verification::delivery::{self, EmailAttachment, NaryDoc};

pub const WHERE: &str = "Nary & AI → Nary and you on WhatsApp";
/// 1 Nov 2026: an MP4 up to this goes to WhatsApp as a playable video.
pub const WA_VIDEO_KEY: &str = "nary.wa.video_max_mb";
/// 1 Nov 2026: how long an email's download link works (days, at most 7: the storage's signing limit).
pub const LINK_DAYS_KEY: &str = "nary.email.link_days";
/// 1 Nov 2026: how long the ERP waits for the WhatsApp service to send one file.
pub const WA_TIMEOUT_KEY: &str = "nary.wa.media_timeout_seconds";
pub const DOC_TYPES: [&str; 3] = ["quotation", "invoice", "sales_document"];
pub const ATTACH_TYPES: [&str; 4] = ["quotation", "invoice", "sales_document", "file"];

/// Where a file's bytes are.
#[derive(Debug, Clone)]
pub enum Stored {
    Object { bucket: String, key: String },
    Inline(Vec<u8>),
}

/// A file Nary may send, found by its id.
#[derive(Debug, Clone)]
pub struct FileRef {
    pub id: Uuid,
    /// attachment | email_attachment | chat_file | knowledge
    pub source: &'static str,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub stored: Stored,
    /// 1 Nov 2026: a video's length (an uploaded attachment's duration_seconds).
    pub duration: Option<f64>,
}

/// A file ready to go out.
#[derive(Debug, Clone)]
pub struct FileBlob {
    pub filename: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// 1 Nov 2026: a file an email carries as a download link (it did not fit the email limit).
#[derive(Debug, Clone)]
pub struct FileLink {
    pub filename: String,
    pub size: i64,
    pub url: String,
    pub days: i64,
}

/// Everything one message carries, in the order given.
#[derive(Debug, Default)]
pub struct Prepared {
    pub docs: Vec<NaryDoc>,
    pub files: Vec<FileBlob>,
    /// email only: the files that go as links
    pub links: Vec<FileLink>,
}

impl Prepared {
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty() && self.files.is_empty() && self.links.is_empty()
    }

    /// The lines an email gets for its linked files (empty when none).
    pub fn link_lines(&self) -> String {
        self.links
            .iter()
            .map(|l| {
                format!(
                    "Download {} ({}, link valid for {} {}): {}",
                    l.filename,
                    human_size(l.size),
                    l.days,
                    if l.days == 1 { "day" } else { "days" },
                    l.url
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The email attachments: documents first (as the ERP sends them), then the files.
    pub fn as_email(&self) -> Vec<EmailAttachment> {
        let mut out: Vec<EmailAttachment> = self
            .docs
            .iter()
            .map(|d| EmailAttachment {
                filename: d.filename.clone(),
                content: d.pdf.clone(),
                content_type: "application/pdf".into(),
                mime_type: "application/pdf".into(),
            })
            .collect();
        out.extend(self.files.iter().map(|f| EmailAttachment {
            filename: f.filename.clone(),
            content: f.data.clone(),
            content_type: f.mime.clone(),
            mime_type: f.mime.clone(),
        }));
        out
    }
}

fn http(status: u16, detail: impl Into<String>) -> ApiError {
    ApiError::http(status, detail.into())
}

/// "1.2 MB", "340 KB", "900 B".
pub fn human_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes + 512) / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn item_type(item: &Value) -> Option<&str> {
    item.get("type").and_then(Value::as_str).map(str::trim)
}

fn item_id(item: &Value) -> String {
    match item.get("id") {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The two caps, from Settings.
pub async fn limits(db: &mut PgConnection) -> ApiResult<(usize, i64)> {
    let files = require_int(db, "nary.attach.max_files", WHERE).await?;
    let mb = require_int(db, "nary.attach.max_total_mb", WHERE).await?;
    Ok((files.max(0) as usize, mb.max(0) * 1024 * 1024))
}

/// May this person open the CRM inbox's email attachments (the inbox tools' rule)?
fn can_read_inbox(user: &AuthUser) -> bool {
    user.is_superadmin || user.perms.contains("crm:view_all") || user.perms.contains("crm:view_own")
}

/// A file id given to Nary, wherever it lives. `Ok(Err(reason))` is told to the model (no such file, or
/// the person may not open it).
pub async fn find_file(
    db: &mut PgConnection,
    user: &AuthUser,
    raw_id: &str,
) -> ApiResult<Result<FileRef, String>> {
    let Ok(id) = Uuid::parse_str(raw_id.trim()) else {
        return Ok(Err(format!(
            "'{raw_id}' is not a file id (find_product_files, list_attachments or the chat give the ids)"
        )));
    };
    // 1. a CRM attachment (AttachmentPanel: organizations, leads, deals, product files, Knowledge Base...)
    #[allow(clippy::type_complexity)]
    let att: Option<(String, String, String, i32, String, String, Option<f64>)> = sqlx::query_as(
        "SELECT entity_type, entity_id, file_name, file_size, mime_type, object_key, duration_seconds \
         FROM attachments WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((etype, eid, name, size, mime, key, duration)) = att {
        if let Err(e) = crate::attachments_access::assert_can_access(db, user, &etype, &eid).await {
            return match e {
                ApiError::Http { .. } => {
                    Ok(Err(format!("You can't send {name}: {}", http_reason(&e))))
                }
                other => Err(other),
            };
        }
        return Ok(Ok(FileRef {
            id,
            source: "attachment",
            filename: name,
            mime,
            size: i64::from(size),
            stored: Stored::Object {
                // a customer file Nary stored (batch 4 D1) stays in the documents bucket
                bucket: crate::storage::attachment_bucket(&key),
                key,
            },
            duration,
        }));
    }
    // 2. an email attachment in the CRM inbox
    let mail: Option<(String, Option<String>, Vec<u8>)> = sqlx::query_as(
        "SELECT filename, content_type, data FROM crm_email_attachments WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((name, mime, data)) = mail {
        if !can_read_inbox(user) {
            return Ok(Err(format!(
                "You can't send {name}: reading the CRM inbox needs crm:view_all or crm:view_own"
            )));
        }
        return Ok(Ok(FileRef {
            id,
            source: "email_attachment",
            filename: if name.is_empty() {
                "attachment".into()
            } else {
                name
            },
            mime: mime
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: data.len() as i64,
            stored: Stored::Inline(data),
            duration: None,
        }));
    }
    // 3. a file sent to Nary in the chat
    if let Some(f) = chat_file(db, user, id).await? {
        return Ok(f);
    }
    // 4. a customer-safe document of the AI knowledge library, with its original file
    let doc: Option<(String, Option<String>, Option<String>, Option<i32>, bool, bool)> =
        sqlx::query_as(
            "SELECT title, original_filename, original_object_key, original_size, customer_safe, is_active \
             FROM ai_knowledge_docs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    if let Some((title, fname, key, size, safe, active)) = doc {
        let Some(key) = key.filter(|k| !k.is_empty()) else {
            return Ok(Err(format!("{title} has no original file to send")));
        };
        if !safe || !active {
            return Ok(Err(format!(
                "{title} is not marked safe for customers (AI → Knowledge), so it is not sent"
            )));
        }
        let name = fname.filter(|f| !f.is_empty()).unwrap_or(title);
        return Ok(Ok(FileRef {
            id,
            source: "knowledge",
            mime: mime_of(&name),
            filename: name,
            size: i64::from(size.unwrap_or(0)),
            stored: Stored::Object {
                bucket: settings().MINIO_BUCKET_DOCUMENTS.clone(),
                key,
            },
            duration: None,
        }));
    }
    Ok(Err(format!("No file with id {id}")))
}

/// A file sent to Nary in the web or WhatsApp chat (batch 3's meeting files; batch 4 D1's chat files):
/// the person who sent it, or Nary's approver, may send it on.
async fn chat_file(
    db: &mut PgConnection,
    user: &AuthUser,
    id: Uuid,
) -> ApiResult<Option<Result<FileRef, String>>> {
    // batch 4 D1's customer files (nary_files): whoever may read them (D1's rule: the source chat, thread,
    // mailbox or a linked record) may send them on
    if let Some(f) = crate::nary::files::load(db, id).await? {
        if let Err(why) = crate::nary::file_tools::may_read(db, user, &f).await? {
            let reason = why
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| why.to_string());
            return Ok(Some(Err(format!(
                "You can't send {}: {reason}",
                f.filename
            ))));
        }
        return Ok(Some(Ok(FileRef {
            id,
            source: "chat_file",
            mime: f
                .mime
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| mime_of(&f.filename)),
            filename: f.filename.clone(),
            size: f.size_bytes,
            stored: Stored::Object {
                bucket: settings().MINIO_BUCKET_DOCUMENTS.clone(),
                key: f.object_key.clone(),
            },
            duration: None,
        })));
    }
    let row: Option<(String, Option<String>, i64, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT filename, mime, size_bytes, object_key, requested_by_id FROM nary_meeting_files WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((name, mime, size, key, by)) = row else {
        return Ok(None);
    };
    let allowed = user.is_superadmin || by == Some(user.id) || user.perms.contains("nary:approve");
    if !allowed {
        return Ok(Some(Err(format!(
            "You can't send {name}: it was sent to Nary by someone else"
        ))));
    }
    Ok(Some(Ok(FileRef {
        id,
        source: "chat_file",
        mime: mime
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| mime_of(&name)),
        filename: name,
        size,
        stored: Stored::Object {
            bucket: settings().MINIO_BUCKET_DOCUMENTS.clone(),
            key,
        },
        duration: None,
    })))
}

fn http_reason(e: &ApiError) -> String {
    match e {
        ApiError::Http { detail, .. } => match detail {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        other => other.to_string(),
    }
}

/// The content type from the extension (for files stored without one).
pub fn mime_of(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// The file's bytes.
pub async fn load_bytes(f: &FileRef) -> ApiResult<Vec<u8>> {
    match &f.stored {
        Stored::Inline(d) => Ok(d.clone()),
        Stored::Object { bucket, key } => crate::storage::minio()
            .get_object(bucket, key)
            .await
            .map_err(|_| http(404, format!("{} is not in storage any more", f.filename))),
    }
}

/// A quotation given by its number (AR-Q/I-2026-10-00002-1) instead of its id: the id, when one matches.
async fn quotation_by_number(db: &mut PgConnection, raw: &str) -> ApiResult<Option<Uuid>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM quotations WHERE (full_number = $1 OR number = $1) AND is_deleted = false \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(raw)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|r| r.0))
}

/// The quotations the Solution Proposals among these items bring with them (delivery::_linked_quotations):
/// (proposal number, quotation id, quotation number). An item that does not load is left to the main check.
async fn bundled_quotations(
    db: &mut PgConnection,
    items: &[Value],
) -> ApiResult<Vec<(String, Uuid, String)>> {
    let adapter = get_adapter("sales_document")?;
    let mut out = Vec::new();
    for item in items
        .iter()
        .filter(|i| item_type(i) == Some("sales_document"))
    {
        let Some(entity) = adapter.load(db, &item_id(item)).await? else {
            continue;
        };
        let proposal = adapter.number(&entity);
        for q in delivery::_linked_quotations(db, "sales_document", &entity).await? {
            let number = q
                .full_number
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| q.number.clone());
            if !out.iter().any(|(_, id, _)| *id == q.id) {
                out.push((proposal.clone(), q.id, number));
            }
        }
    }
    Ok(out)
}

/// 1 Nov 2026: how the WhatsApp channel sends a file: "photo", "video" (an MP4 up to `wa_video_max`) or
/// "document".
pub fn whatsapp_kind(mime: &str, size: i64, wa_video_max: i64) -> &'static str {
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if crate::nary::uploads::is_picture(&base) {
        "photo"
    } else if base == "video/mp4" && size <= wa_video_max {
        "video"
    } else {
        "document"
    }
}

/// 1 Nov 2026: the bytes for a setting in MB (a missing one is an error naming the key).
async fn mb_setting(db: &mut PgConnection, key: &str) -> ApiResult<i64> {
    Ok(require_int(db, key, WHERE).await?.max(0) * 1024 * 1024)
}

/// 1 Nov 2026: the days an email's download link works (nary.email.link_days, 1 to 7).
pub async fn link_days(db: &mut PgConnection) -> ApiResult<i64> {
    Ok(require_int(db, LINK_DAYS_KEY, WHERE).await?.clamp(1, 7))
}

/// 1 Nov 2026: what a video file on a card says about itself: its length, then how it goes.
fn video_note(f: &FileRef, how: &str) -> String {
    let mut parts = vec![human_size(f.size)];
    if let Some(d) = f.duration {
        parts.push(crate::nary::uploads::duration_label(d));
    }
    parts.push(crate::nary::uploads::type_label(&f.mime));
    if !how.is_empty() {
        parts.push(how.to_string());
    }
    format!("{} ({})", f.filename, parts.join(", "))
}

/// 5 Nov 2026: how a video goes (the CRM attachment rules, also for a library video): on email "attached" or
/// "link" (over the email limit), on WhatsApp "video" (an MP4 up to nary.wa.video_max_mb) or "document";
/// and the card's words for it. `Ok(Err)` is a video over the upload cap.
async fn video_send_as(
    db: &mut PgConnection,
    f: &FileRef,
    email: bool,
    file_bytes: i64,
    max_bytes: i64,
) -> ApiResult<Result<(&'static str, String), String>> {
    if email {
        if matches!(f.stored, Stored::Object { .. }) && file_bytes + f.size > max_bytes {
            let days = link_days(db).await?;
            return Ok(Ok((
                "link",
                format!(
                    "sent as a download link valid {days} {}: over the {} an email carries",
                    if days == 1 { "day" } else { "days" },
                    human_size(max_bytes)
                ),
            )));
        }
        return Ok(Ok(("attached", "attached to the email".to_string())));
    }
    let cap = match crate::nary::uploads::video_max_bytes(db).await? {
        Ok(c) => c,
        Err(why) => return Ok(Err(why)),
    };
    if f.size > cap {
        return Ok(Err(format!(
            "{} is {}, over the {} a video may be ({}).",
            f.filename,
            human_size(f.size),
            human_size(cap),
            crate::nary::uploads::VIDEO_MAX_KEY
        )));
    }
    let wa_max = mb_setting(db, WA_VIDEO_KEY).await?;
    let kind = whatsapp_kind(&f.mime, f.size, wa_max);
    let how = if kind == "video" {
        "goes as a playable video".to_string()
    } else if f.mime.starts_with("video/mp4") {
        format!(
            "goes as a document: over the {} WhatsApp plays as a video",
            human_size(wa_max)
        )
    } else {
        "goes as a document: WhatsApp plays only MP4".to_string()
    };
    Ok(Ok((kind, how)))
}

/// The attach list for the card: each item checked now (it exists, the person may send it, the caps),
/// as clean `{type, id}` items and one label each ("Quotation AR-Q-I-2026-10-00002-1.pdf (PDF, approved)",
/// "Acme - Edge Gateway - Datasheet.pdf (1.2 MB)"), plus warnings for the card. `Ok(Err(reason))` is told
/// to the model.
///
/// 23 Oct: a Solution Proposal's linked quotations go with it as their own PDFs, so the card lists them
/// ("... (PDF, from the proposal)"); a quotation also given on its own (or an item given twice) is dropped with
/// a warning, so each PDF goes once.
pub async fn card_items(
    db: &mut PgConnection,
    user: &AuthUser,
    items: &[Value],
    // doclib A3: who the message goes to (library documents follow the send rules for them)
    to: Option<&crate::nary::document_send::Recipient>,
    // 1 Nov 2026: "whatsapp" or "email" (videos and email links)
    channel: &str,
) -> ApiResult<Result<(Vec<Value>, Vec<String>, Vec<String>), String>> {
    let (max_files, max_bytes) = limits(db).await?;
    let email = channel == "email";
    if items.len() > max_files {
        return Ok(Err(format!(
            "At most {max_files} files go with one message (nary.attach.max_files); send the rest in another message."
        )));
    }
    let mut clean = Vec::new();
    let mut labels = Vec::new();
    let mut warnings = Vec::new();
    let mut file_bytes: i64 = 0;
    let bundled = bundled_quotations(db, items).await?;
    let mut given: Vec<(String, String)> = Vec::new();
    for item in items {
        let Some(t) = item_type(item).filter(|t| ATTACH_TYPES.contains(t)) else {
            // Python's text for a bad type (a file is the fourth type, in the tool's schema)
            return Ok(Err(format!(
                "Only {} can be attached",
                delivery::ATTACHABLE.join(", ")
            )));
        };
        let mut id = item_id(item);
        if id.is_empty() {
            return Ok(Err(format!("The {t} to attach needs its id")));
        }
        if t == "file" {
            // doclib A3: a library document ("doc:<id>"), or a catalog product file the library took over
            if let Some(did) = library_doc(db, &id).await? {
                use crate::nary::document_send as ds;
                let item = match ds::resolve(db, user, did, to).await? {
                    Ok(i) => i,
                    Err(why) => return Ok(Err(why)),
                };
                let Some(rcpt) = to else {
                    return Ok(Err(format!("{} needs a known recipient", item.doc.title)));
                };
                let mut entry = json!({"type": "file", "id": ds::attach_id(did), "name": item.version.file_name});
                // 5 Nov 2026: a library video goes the way a CRM attachment video goes
                if crate::doc_library::is_video(&item.version) {
                    let v = &item.version;
                    let probe = FileRef {
                        id: v.id,
                        source: "library",
                        filename: v.file_name.clone(),
                        mime: v.file_mime.clone(),
                        size: v.file_size,
                        stored: Stored::Object {
                            bucket: crate::doc_library::bucket_name(&v.file_bucket),
                            key: v.file_object_key.clone(),
                        },
                        duration: v.duration_seconds,
                    };
                    let (send_as, how) =
                        match video_send_as(db, &probe, email, file_bytes, max_bytes).await? {
                            Ok(x) => x,
                            Err(why) => return Ok(Err(why)),
                        };
                    if send_as != "link" && email {
                        file_bytes += v.file_size;
                    }
                    labels.push(format!("{}; {how}", item.label(rcpt)));
                    entry["send_as"] = json!(send_as);
                    clean.push(entry);
                    continue;
                }
                file_bytes += item.version.file_size;
                labels.push(item.label(rcpt));
                clean.push(entry);
                continue;
            }
            let f = match find_file(db, user, &id).await? {
                Ok(f) => f,
                Err(why) => return Ok(Err(why)),
            };
            let video = crate::nary::uploads::is_video(&f.mime);
            // 1 Nov 2026: how it goes, said on the card
            let mut send_as: Option<&str> = None;
            let mut how = String::new();
            if email {
                if matches!(f.stored, Stored::Object { .. }) && file_bytes + f.size > max_bytes {
                    let days = link_days(db).await?;
                    send_as = Some("link");
                    how = format!(
                        "sent as a download link valid {days} {}: over the {} an email carries",
                        if days == 1 { "day" } else { "days" },
                        human_size(max_bytes)
                    );
                } else {
                    file_bytes += f.size;
                }
            } else if video {
                let cap = match crate::nary::uploads::video_max_bytes(db).await? {
                    Ok(c) => c,
                    Err(why) => return Ok(Err(why)),
                };
                if f.size > cap {
                    return Ok(Err(format!(
                        "{} is {}, over the {} a video may be ({}).",
                        f.filename,
                        human_size(f.size),
                        human_size(cap),
                        crate::nary::uploads::VIDEO_MAX_KEY
                    )));
                }
                let wa_max = mb_setting(db, WA_VIDEO_KEY).await?;
                let kind = whatsapp_kind(&f.mime, f.size, wa_max);
                send_as = Some(kind);
                how = if kind == "video" {
                    "goes as a playable video".to_string()
                } else if f.mime.starts_with("video/mp4") {
                    format!(
                        "goes as a document: over the {} WhatsApp plays as a video",
                        human_size(wa_max)
                    )
                } else {
                    "goes as a document: WhatsApp plays only MP4".to_string()
                };
            } else {
                file_bytes += f.size;
            }
            if video || send_as.is_some() {
                labels.push(video_note(&f, &how));
            } else {
                // 29 Oct 2026: the type too (over MCP the card text is all the person sees)
                labels.push(format!(
                    "{} ({}, {})",
                    f.filename,
                    human_size(f.size),
                    crate::nary::uploads::type_label(&f.mime)
                ));
            }
            let mut entry = json!({"type": "file", "id": f.id.to_string(), "name": f.filename});
            if let Some(a) = send_as {
                entry["send_as"] = json!(a);
            }
            clean.push(entry);
            continue;
        }
        if t == "quotation" && Uuid::parse_str(&id).is_err() {
            match quotation_by_number(db, &id).await? {
                Some(qid) => id = qid.to_string(),
                None => return Ok(Err(format!("No quotation with id or number {id}"))),
            }
        }
        if t == "quotation" {
            let qid = Uuid::parse_str(&id).ok();
            if let Some((proposal, _, number)) = bundled.iter().find(|(_, q, _)| Some(*q) == qid) {
                warnings.push(format!(
                    "Quotation {number} is already included with the proposal {proposal}; it goes once."
                ));
                continue;
            }
        }
        let key = (t.to_string(), id.to_lowercase());
        if given.contains(&key) {
            warnings.push(format!("The {t} {id} was given twice; it goes once."));
            continue;
        }
        given.push(key);
        let perm = delivery::SENDABLE
            .iter()
            .find(|(k, _)| *k == t)
            .map(|(_, p)| *p)
            .unwrap_or_default();
        if !user.is_superadmin && !user.perms.contains(perm) {
            return Ok(Err(format!("Sending a {t} needs the permission {perm}")));
        }
        let probe = json!({"type": t, "id": id});
        let label = match delivery::attachment_label(db, &probe).await? {
            Ok(l) => l,
            Err(e) => return Ok(Err(e)),
        };
        let adapter = get_adapter(t)?;
        let Some(entity) = adapter.load(db, &id).await? else {
            return Ok(Err(format!("No {t} with id {id}")));
        };
        let status = entity.status().unwrap_or("").to_string();
        if crate::verification::service::VOID_STATUSES.contains(&status.as_str())
            || matches!(status.as_str(), "superseded" | "rejected")
        {
            return Ok(Err(format!("A {status} {t} can't be sent")));
        }
        let approved = crate::approvals::engine::is_approved(db, t, &entity).await?;
        let state = if approved {
            "PDF, approved".to_string()
        } else {
            "PDF, not approved yet: it goes for approval when you confirm and the message waits"
                .to_string()
        };
        labels.push(format!("{label} ({state})"));
        clean.push(json!({"type": t, "id": id}));
        if t == "sales_document" {
            let proposal = adapter.number(&entity);
            for (_, q, _) in bundled.iter().filter(|(p, _, _)| *p == proposal) {
                let probe = json!({"type": "quotation", "id": q.to_string()});
                if let Ok(l) = delivery::attachment_label(db, &probe).await? {
                    labels.push(format!("{l} (PDF, from the proposal)"));
                }
            }
        }
    }
    if file_bytes > max_bytes {
        return Ok(Err(format!(
            "The files come to {}, over the {} a message may carry (nary.attach.max_total_mb).",
            human_size(file_bytes),
            human_size(max_bytes)
        )));
    }
    Ok(Ok((clean, labels, warnings)))
}

/// 29 Oct 2026: the files of a card's clean attach list (not the sales documents or library documents, which the
/// labels describe) for the card UI: id, name, size, type, whether WhatsApp shows it as a photo, and for a CRM
/// attachment that is a picture the authenticated download path its thumbnail loads from.
pub async fn card_files(
    db: &mut PgConnection,
    user: &AuthUser,
    clean: &[Value],
) -> ApiResult<Vec<Value>> {
    let mut out = Vec::new();
    for item in clean.iter().filter(|i| item_type(i) == Some("file")) {
        let id = item_id(item);
        if library_doc(db, &id).await?.is_some() {
            continue;
        }
        let Ok(f) = find_file(db, user, &id).await? else {
            continue;
        };
        let picture = crate::nary::uploads::is_picture(&f.mime);
        let thumbnail = (picture && f.source == "attachment")
            .then(|| format!("/api/v1/attachments/{}/download", f.id));
        let video = crate::nary::uploads::is_video(&f.mime);
        let mut extra = serde_json::Map::new();
        if video {
            // 1 Nov 2026: the card shows size and length
            extra.insert("duration_seconds".into(), json!(f.duration));
            extra.insert(
                "duration_label".into(),
                json!(f.duration.map(crate::nary::uploads::duration_label)),
            );
        }
        if let Some(a) = item.get("send_as").and_then(Value::as_str) {
            extra.insert("send_as".into(), json!(a));
        }
        let mut entry = json!({
            "id": f.id.to_string(),
            "name": f.filename,
            "size": f.size,
            "size_label": human_size(f.size),
            "mime_type": f.mime,
            "type": crate::nary::uploads::type_label(&f.mime),
            "picture": picture,
            "video": video,
            "source": f.source,
            "thumbnail_url": thumbnail,
        });
        if let Value::Object(m) = &mut entry {
            m.extend(extra);
        }
        out.push(entry);
    }
    Ok(out)
}

/// doclib A3: a library document id: `doc:<id>`, or a batch 4 catalog product file the library took over.
pub async fn library_doc(db: &mut PgConnection, raw_id: &str) -> ApiResult<Option<Uuid>> {
    if let Some(id) = crate::nary::document_send::doc_ref(raw_id) {
        return Ok(Some(id));
    }
    match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => crate::nary::document_send::migrated_document(db, id).await,
        Err(_) => Ok(None),
    }
}

/// At the send: the bytes of every item, documents through the approval gate (409 when one isn't
/// approved: nothing is sent). Errors are HTTP errors with the reason. Without a recipient no library
/// document can go (see [`prepare_for`]).
pub async fn prepare(
    db: &mut PgConnection,
    user: &AuthUser,
    items: &[Value],
) -> ApiResult<Prepared> {
    prepare_for(db, user, items, None, "whatsapp").await
}

/// doclib A3: [`prepare`] for a known recipient: library documents are checked against the send rules
/// again and go as the customer's stamped copy (doc_library::deliver::customer_copy).
/// 1 Nov 2026: `channel` "email" turns stored files over the email limit into download links (as the card
/// said); "whatsapp" sends videos on their own cap.
pub async fn prepare_for(
    db: &mut PgConnection,
    user: &AuthUser,
    items: &[Value],
    to: Option<&crate::nary::document_send::Recipient>,
    channel: &str,
) -> ApiResult<Prepared> {
    let email = channel == "email";
    let (max_files, max_bytes) = limits(db).await?;
    if items.len() > max_files {
        return Err(http(
            422,
            format!("At most {max_files} files go with one message (nary.attach.max_files)"),
        ));
    }
    let docs: Vec<Value> = items
        .iter()
        .filter(|i| item_type(i).is_some_and(|t| DOC_TYPES.contains(&t)))
        .cloned()
        .collect();
    let mut out = Prepared {
        docs: delivery::nary_documents(db, &docs, user).await?,
        files: vec![],
        links: vec![],
    };
    let mut total: i64 = out.docs.iter().map(|d| d.pdf.len() as i64).sum();
    // the files attached so far (the card's rule for links: documents are not known there)
    let mut file_total: i64 = 0;
    for item in items {
        match item_type(item) {
            Some("file") => {}
            Some(t) if DOC_TYPES.contains(&t) => continue,
            _ => {
                return Err(http(
                    422,
                    format!("An attachment's type is one of {}", ATTACH_TYPES.join(", ")),
                ))
            }
        }
        // doclib A3: a library document goes as the recipient's stamped copy
        if let Some(did) = library_doc(db, &item_id(item)).await? {
            use crate::nary::document_send as ds;
            let doc = match ds::resolve(db, user, did, to).await? {
                Ok(i) => i,
                Err(why) => return Err(http(403, why)),
            };
            let Some(rcpt) = to else {
                return Err(http(
                    403,
                    format!("{} needs a known recipient", doc.doc.title),
                ));
            };
            // 5 Nov 2026: a library video: logged for the recipient (the original bytes, no watermark), then
            // the CRM attachment video rules: an email link over the email limit, on WhatsApp on its own cap
            if crate::doc_library::is_video(&doc.version) {
                let v = &doc.version;
                if !email {
                    let cap = match crate::nary::uploads::video_max_bytes(db).await? {
                        Ok(c) => c,
                        Err(why) => return Err(http(422, why)),
                    };
                    if v.file_size > cap {
                        return Err(http(
                            413,
                            format!(
                                "{} is {}, over the {} a video may be ({})",
                                v.file_name,
                                human_size(v.file_size),
                                human_size(cap),
                                crate::nary::uploads::VIDEO_MAX_KEY
                            ),
                        ));
                    }
                }
                let blob = ds::customer_blob(db, user, &doc, rcpt).await?;
                if email && file_total + v.file_size > max_bytes {
                    let days = link_days(db).await?;
                    let url = crate::storage::browser_link(
                        &crate::doc_library::bucket_name(&v.file_bucket),
                        &v.file_object_key,
                        days * 24 * 3600,
                        Some(&v.file_mime),
                        Some(&v.file_name),
                    )
                    .map_err(|e| {
                        http(
                            502,
                            format!("No download link for {}: {}", v.file_name, e.message),
                        )
                    })?;
                    out.links.push(FileLink {
                        filename: v.file_name.clone(),
                        size: v.file_size,
                        url,
                        days,
                    });
                    continue;
                }
                if email {
                    total += blob.data.len() as i64;
                    file_total += blob.data.len() as i64;
                }
                out.files.push(blob);
                continue;
            }
            let blob = ds::customer_blob(db, user, &doc, rcpt).await?;
            total += blob.data.len() as i64;
            file_total += blob.data.len() as i64;
            out.files.push(blob);
            continue;
        }
        let f = match find_file(db, user, &item_id(item)).await? {
            Ok(f) => f,
            Err(why) => return Err(http(403, why)),
        };
        // 1 Nov 2026: an email link for a stored file that does not fit
        if email && file_total + f.size > max_bytes {
            if let Stored::Object { bucket, key } = &f.stored {
                let days = link_days(db).await?;
                let url = crate::storage::browser_link(
                    bucket,
                    key,
                    days * 24 * 3600,
                    Some(&f.mime),
                    Some(&f.filename),
                )
                .map_err(|e| {
                    http(
                        502,
                        format!("No download link for {}: {}", f.filename, e.message),
                    )
                })?;
                out.links.push(FileLink {
                    filename: f.filename,
                    size: f.size,
                    url,
                    days,
                });
                continue;
            }
        }
        // 1 Nov 2026: a video on WhatsApp goes on its own, up to the video upload cap
        if !email && crate::nary::uploads::is_video(&f.mime) {
            let cap = match crate::nary::uploads::video_max_bytes(db).await? {
                Ok(c) => c,
                Err(why) => return Err(http(422, why)),
            };
            if f.size > cap {
                return Err(http(
                    413,
                    format!(
                        "{} is {}, over the {} a video may be ({})",
                        f.filename,
                        human_size(f.size),
                        human_size(cap),
                        crate::nary::uploads::VIDEO_MAX_KEY
                    ),
                ));
            }
            let data = load_bytes(&f).await?;
            out.files.push(FileBlob {
                filename: f.filename,
                mime: f.mime,
                data,
            });
            continue;
        }
        let data = load_bytes(&f).await?;
        total += data.len() as i64;
        file_total += f.size;
        out.files.push(FileBlob {
            filename: f.filename,
            mime: f.mime,
            data,
        });
    }
    if total > max_bytes {
        return Err(http(
            413,
            format!(
                "The attachments come to {}, over the {} a message may carry (nary.attach.max_total_mb)",
                human_size(total),
                human_size(max_bytes)
            ),
        ));
    }
    Ok(out)
}

/// The WhatsApp caption of a document: its number and the link that verifies it.
pub fn doc_caption(d: &NaryDoc) -> String {
    match &d.verify_url {
        Some(link) => format!("{} {}\nVerify this document: {link}", d.label, d.number),
        None => format!("{} {}", d.label, d.number),
    }
}

/// After the text went into the chat: each file as a WhatsApp document (a picture as a photo), stored as Nary's
/// message.
/// Returns what went (one entry per file). A failure stops there and says which files already went.
pub async fn send_whatsapp(
    db: &mut PgConnection,
    chat_id: &str,
    prepared: &Prepared,
) -> ApiResult<Vec<Value>> {
    let base = settings().WHATSAPP_SERVICE_URL.clone();
    if base.is_empty() {
        return Err(http(503, "WhatsApp service not configured"));
    }
    let mut queue: Vec<(String, String, &[u8], String)> = prepared
        .docs
        .iter()
        .map(|d| {
            (
                d.filename.clone(),
                "application/pdf".to_string(),
                d.pdf.as_slice(),
                doc_caption(d),
            )
        })
        .collect();
    queue.extend(prepared.files.iter().map(|f| {
        (
            f.filename.clone(),
            f.mime.clone(),
            f.data.as_slice(),
            String::new(),
        )
    }));
    // 1 Nov 2026: an MP4 up to this is a playable video; a file may take minutes (a 100 MB video)
    let wa_video_max = mb_setting(db, WA_VIDEO_KEY).await?;
    let wait = require_int(db, WA_TIMEOUT_KEY, WHERE).await?.max(1) as u64;
    let mut sent: Vec<Value> = Vec::new();
    for (filename, mime, data, caption) in queue {
        // 29 Oct 2026: the sidecar sends an image type as a photo (whatsapp-web.js without sendMediaAsDocument);
        // a PDF or an Office file goes as a document. 1 Nov 2026: an MP4 up to nary.wa.video_max_mb as a video
        // (an HEVC one would not play: a document), any other video with asDocument.
        let mut kind = whatsapp_kind(&mime, data.len() as i64, wa_video_max);
        if kind == "video" && !crate::nary::uploads::mp4_plays_on_whatsapp(data) {
            kind = "document";
        }
        let mut payload = json!({
            "to": chat_id,
            "data": granary_compat::crypto::b64_std(data),
            "mimetype": mime,
            "filename": filename,
            "caption": caption,
        });
        if kind == "document" && crate::nary::uploads::is_video(&mime) {
            payload["asDocument"] = json!(true);
        }
        let resp = crate::nary::wa_send::wa_post(
            &format!("{base}/send-media"),
            &payload,
            Duration::from_secs(wait),
        )
        .await;
        let resp = match resp {
            Ok(r) if r.status == 200 => r,
            Ok(r) => {
                return Err(partial(
                    &sent,
                    &filename,
                    crate::nary::wa_send::raise_wa_error(&r),
                ))
            }
            Err(e) => return Err(partial(&sent, &filename, e)),
        };
        let body = resp.json().unwrap_or(Value::Null);
        let serialized = body
            .get("messageId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let sent_chat = body
            .get("chatId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(chat_id)
            .to_string();
        store_document(
            db,
            chat_id,
            &sent_chat,
            serialized.as_deref(),
            &filename,
            &mime,
            data.len(),
            &caption,
            match kind {
                "photo" => "image",
                "video" => "video",
                _ => "document",
            },
        )
        .await?;
        crate::nary::outbound_log::record(
            db,
            crate::nary::outbound_log::Outbound {
                channel: "whatsapp",
                chat_id: Some(chat_id),
                kind: match kind {
                    "photo" => "image",
                    "video" => "video",
                    _ => "document",
                },
                message_ref: serialized.as_deref(),
                ..Default::default()
            },
        )
        .await;
        sent.push(json!({"filename": filename, "message_id": serialized, "as": kind}));
    }
    crate::nary::commit(db).await?;
    Ok(sent)
}

/// The error of a file that could not go, naming the ones that already went.
fn partial(sent: &[Value], filename: &str, e: ApiError) -> ApiError {
    let reason = http_reason(&e);
    let went: Vec<String> = sent
        .iter()
        .filter_map(|s| {
            s.get("filename")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let before = if went.is_empty() {
        "The message text went out".to_string()
    } else {
        format!("The message text and {} went out", went.join(", "))
    };
    http(
        502,
        format!("{before}, but {filename} could not be sent: {reason}"),
    )
}

/// A document (or, 29 Oct 2026, a photo; 1 Nov 2026, a video) Nary sent, kept in the chat like its text messages (sent_by_nary; the
/// echo upserts it).
#[allow(clippy::too_many_arguments)]
async fn store_document(
    db: &mut PgConnection,
    chat_id: &str,
    sent_chat_id: &str,
    serialized: Option<&str>,
    filename: &str,
    mime: &str,
    size: usize,
    caption: &str,
    // 1 Nov 2026: the wa_messages type: image, video or document
    kind: &str,
) -> ApiResult<()> {
    let Some(key) = crate::wa_signature::message_key(serialized) else {
        return Ok(());
    };
    let chat = crate::nary::wa_send::load_chat(db, chat_id).await?;
    let Some(chat) = chat else {
        return Ok(());
    };
    let body = if caption.is_empty() {
        filename
    } else {
        caption
    };
    crate::nary::wa_send::store_sent(db, &chat, &key, serialized, body, Utc::now(), sent_chat_id)
        .await?;
    sqlx::query(
        "UPDATE wa_messages SET type = $5, has_media = true, media_name = $2, media_mime = $3, \
         media_size = $4 WHERE message_key = $1",
    )
    .bind(&key)
    .bind(filename)
    .bind(mime)
    .bind(i32::try_from(size).unwrap_or(i32::MAX))
    .bind(kind)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "UPDATE wa_sent_messages SET kind = 'reply', card_id = $2, updated_at = now() WHERE message_key = $1",
    )
    .bind(&key)
    .bind(crate::nary::wa_send::current_card())
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// After the message went: its documents are marked sent as the ERP's Send marks them.
pub async fn mark_sent(
    db: &mut PgConnection,
    user: &AuthUser,
    prepared: &mut Prepared,
    channel: &str,
    to: &str,
    message_id: Option<&str>,
) -> ApiResult<()> {
    if prepared.docs.is_empty() {
        return Ok(());
    }
    delivery::nary_mark_sent(db, &mut prepared.docs, user, channel, to, message_id).await
}
