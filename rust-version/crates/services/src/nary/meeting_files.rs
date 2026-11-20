//! Nary batch 3 (R1, Rust only): meeting recordings and notes files.
//!
//! ```text
//!     a file for a meeting     Nary → Meetings "Upload recording" (save_upload with the meeting), or a file
//!                              the Owner sends Nary on WhatsApp (from_whatsapp) or in the web chat
//!                              (save_upload without a meeting): stored privately (documents bucket), one
//!                              nary_meeting_files row. A chat file waits as 'stored' until Nary knows the
//!                              meeting: attach_meeting_notes (the Owner names it, or their latest past meeting
//!                              with that customer).
//!     tick()                   every minute (the playbooks tick): a notes file is read at once; a recording
//!                              goes to the speech-to-text job API (services/stt POST /jobs, one job at a
//!                              time there), is polled every nary.meetings.recording_poll_seconds (progress),
//!                              and when the transcript is in, the batch 2 debrief runs with it (results
//!                              note on the deal, follow-ups, the stage/value card, the summary email card).
//!                              The Owner gets one WhatsApp message (through the Owner channel, bundled with the
//!                              stage/value card); a failure is told with the reason.
//! ```
//!
//! Statuses: stored (chat file, no meeting yet) -> queued -> transcribing (recordings) -> analysing -> done |
//! failed. Each step claims the row with a conditional UPDATE, so two ticks never run the same file.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::config::settings;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::nary::{NaryMeeting, WaChat, WaMessage};

use crate::nary::{commit, meetings, owner_channel, roles, rollback};
use crate::storage::{ensure_bucket, minio};
use crate::stt_client::{self, JobError};

pub const WHERE: &str = "Nary & AI → Meetings";
pub const NOTES_EXTS: [&str; 5] = [".txt", ".srt", ".vtt", ".docx", ".md"];
pub const RECORDING_EXTS: [&str; 7] = [".mp3", ".m4a", ".wav", ".ogg", ".opus", ".webm", ".mp4"];
/// Statuses of a file Nary is still working on.
pub const WORKING: [&str; 3] = ["queued", "transcribing", "analysing"];

const COLS: &str = "id, created_at, updated_at, kind, meeting_id, deal_id, filename, mime, size_bytes, object_key, \
    source, wa_message_key, status, progress, error, stt_job_id, submits, language, duration_seconds, transcript, \
    result, requested_by_id, next_poll_at, started_at, finished_at";

/// One row of nary_meeting_files.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MeetingFile {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// notes | recording
    pub kind: String,
    pub meeting_id: Option<Uuid>,
    pub deal_id: Option<Uuid>,
    pub filename: String,
    pub mime: Option<String>,
    pub size_bytes: i64,
    pub object_key: String,
    /// upload | whatsapp | web_chat
    pub source: String,
    pub wa_message_key: Option<String>,
    pub status: String,
    pub progress: i32,
    pub error: Option<String>,
    pub stt_job_id: Option<String>,
    pub submits: i32,
    pub language: Option<String>,
    pub duration_seconds: Option<f64>,
    pub transcript: Option<String>,
    pub result: Option<Value>,
    pub requested_by_id: Option<Uuid>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

fn iso(v: Option<&DateTime<Utc>>) -> Value {
    match v {
        Some(t) => json!(granary_compat::pyjson::PyDateTime::from_chrono_utc(t).isoformat()),
        None => Value::Null,
    }
}

/// What the pages and the tools show of a file (never the transcript itself: it is downloaded).
pub fn view(f: &MeetingFile) -> Value {
    let summary = f
        .result
        .as_ref()
        .and_then(|r| r.get("extract"))
        .and_then(|e| e.get("summary"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "id": f.id.to_string(), "kind": f.kind, "meeting_id": f.meeting_id.map(|v| v.to_string()),
        "deal_id": f.deal_id.map(|v| v.to_string()), "filename": f.filename, "size_bytes": f.size_bytes,
        "source": f.source, "status": f.status, "progress": f.progress, "error": f.error,
        "language": f.language, "duration_seconds": f.duration_seconds,
        "has_transcript": f.transcript.as_deref().is_some_and(|t| !t.is_empty()),
        "summary": summary,
        "deal_update_card": f.result.as_ref().and_then(|r| r.get("deal_update_card")).cloned().unwrap_or(Value::Null),
        "created_at": iso(Some(&f.created_at)), "finished_at": iso(f.finished_at.as_ref()),
    })
}

fn ext_of(filename: &str) -> String {
    let lower = filename.to_lowercase();
    match lower.rfind('.') {
        Some(i) if i + 1 < lower.len() => lower[i..].to_string(),
        _ => String::new(),
    }
}

/// notes | recording, from the file name (then the mime type); None for anything else.
pub fn kind_of(filename: &str, mime: Option<&str>) -> Option<&'static str> {
    let ext = ext_of(filename);
    if NOTES_EXTS.contains(&ext.as_str()) {
        return Some("notes");
    }
    if RECORDING_EXTS.contains(&ext.as_str()) {
        return Some("recording");
    }
    let m = mime
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if m.starts_with("audio/") || m.starts_with("video/") {
        return Some("recording");
    }
    if matches!(
        m.as_str(),
        "text/plain"
            | "text/vtt"
            | "text/markdown"
            | "application/x-subrip"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    ) {
        return Some("notes");
    }
    None
}

/// The file name a notes reader needs (its extension decides txt / srt / vtt / docx).
fn notes_name(f: &MeetingFile) -> String {
    if NOTES_EXTS.contains(&ext_of(&f.filename).as_str()) {
        return f.filename.clone();
    }
    match f.mime.as_deref().unwrap_or("") {
        "text/vtt" => format!("{}.vtt", f.filename),
        "application/x-subrip" => format!("{}.srt", f.filename),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            format!("{}.docx", f.filename)
        }
        _ => format!("{}.txt", f.filename),
    }
}

pub async fn load(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<MeetingFile>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM nary_meeting_files WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn load_meeting(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<NaryMeeting>> {
    Ok(sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// The files of a meeting, or the newest files (`meeting_id` None), newest first.
pub async fn list(
    db: &mut PgConnection,
    meeting_id: Option<Uuid>,
    limit: i64,
) -> ApiResult<Vec<MeetingFile>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM nary_meeting_files WHERE ($1::uuid IS NULL OR meeting_id = $1) \
         ORDER BY created_at DESC LIMIT $2"
    ))
    .bind(meeting_id)
    .bind(limit)
    .fetch_all(&mut *db)
    .await?)
}

/// The size limit of a kind, in MB (settings).
pub async fn max_mb(db: &mut PgConnection, kind: &str) -> ApiResult<i64> {
    let key = if kind == "recording" {
        "nary.meetings.recording_max_mb"
    } else {
        "nary.meetings.notes_max_mb"
    };
    Ok(require_int(db, key, WHERE).await?)
}

async fn check_size(db: &mut PgConnection, kind: &str, size: usize) -> ApiResult<()> {
    let mb = max_mb(db, kind).await?;
    if size as i64 > mb * 1024 * 1024 {
        let what = if kind == "recording" {
            "recording"
        } else {
            "notes file"
        };
        return Err(ApiError::http(
            413,
            format!("The {what} is too large ({mb} MB at most)"),
        ));
    }
    Ok(())
}

fn bucket() -> String {
    settings().MINIO_BUCKET_DOCUMENTS.clone()
}

async fn put(key: &str, data: &[u8], mime: Option<&str>) -> ApiResult<()> {
    let store = minio();
    let b = bucket();
    ensure_bucket(&*store, &b)
        .await
        .map_err(ApiError::internal)?;
    store
        .put_object(
            &b,
            key,
            data,
            mime.filter(|m| !m.is_empty())
                .unwrap_or("application/octet-stream"),
        )
        .await
        .map_err(ApiError::internal)
}

async fn read(key: &str) -> ApiResult<Vec<u8>> {
    minio()
        .get_object(&bucket(), key)
        .await
        .map_err(ApiError::internal)
}

/// A file uploaded on Nary → Meetings (with its meeting: queued at once) or in the web chat (no meeting
/// yet: stored until attach_meeting_notes). 415 for a file that is neither notes nor a recording, 413 over
/// the size limit.
pub async fn save_upload(
    db: &mut PgConnection,
    user: &AuthUser,
    meeting: Option<&NaryMeeting>,
    filename: &str,
    mime: Option<&str>,
    data: &[u8],
    source: &str,
) -> ApiResult<MeetingFile> {
    let Some(kind) = kind_of(filename, mime) else {
        return Err(ApiError::http(
            415,
            "Use a recording (mp3, m4a, wav, ogg, opus, webm, mp4) or a notes file (txt, docx, srt, vtt)",
        ));
    };
    if data.is_empty() {
        return Err(ApiError::http(400, "The file is empty"));
    }
    check_size(db, kind, data.len()).await?;
    let id = Uuid::new_v4();
    let key = format!("nary/meetings/{id}{}", ext_of(filename));
    put(&key, data, mime).await?;
    let name: String = filename.chars().take(255).collect();
    let (status, meeting_id, deal_id) = match meeting {
        Some(m) => ("queued", Some(m.id), m.deal_id),
        None => ("stored", None, None),
    };
    let row: MeetingFile = sqlx::query_as(&format!(
        "INSERT INTO nary_meeting_files (id, kind, meeting_id, deal_id, filename, mime, size_bytes, object_key, \
         source, status, requested_by_id, next_poll_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now()) \
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(kind)
    .bind(meeting_id)
    .bind(deal_id)
    .bind(&name)
    .bind(mime.map(|m| m.chars().take(120).collect::<String>()))
    .bind(data.len() as i64)
    .bind(&key)
    .bind(source)
    .bind(status)
    .bind(user.id)
    .fetch_one(&mut *db)
    .await?;
    Ok(row)
}

/// The most pieces one upload may come in.
const MAX_PARTS: i64 = 1000;

fn bad_part(msg: &str) -> ApiError {
    ApiError::http(400, msg.to_string())
}

/// Piece `index` (from 0) of `total` of a large file sent in pieces: a request through Cloudflare is cut
/// at 100 MB, so the pages send a recording in pieces of a few tens of MB. The pieces wait in the
/// documents bucket (under the person's id) until the last one; then the whole file is saved as
/// [`save_upload`] saves it and the pieces are removed. None until the last piece.
#[allow(clippy::too_many_arguments)]
pub async fn save_part(
    db: &mut PgConnection,
    user: &AuthUser,
    meeting: Option<&NaryMeeting>,
    upload_id: Uuid,
    index: i64,
    total: i64,
    filename: &str,
    mime: Option<&str>,
    data: &[u8],
    source: &str,
) -> ApiResult<Option<MeetingFile>> {
    if total == 1 && index == 0 {
        return save_upload(db, user, meeting, filename, mime, data, source)
            .await
            .map(Some);
    }
    let Some(kind) = kind_of(filename, mime) else {
        return save_upload(db, user, meeting, filename, mime, data, source)
            .await
            .map(Some); // the same 415
    };
    if !(1..=MAX_PARTS).contains(&total) || !(0..total).contains(&index) {
        return Err(bad_part(
            "This piece of the upload is not valid; upload the file again",
        ));
    }
    check_size(db, kind, data.len()).await?;
    let prefix = format!("nary/uploads/{}/{upload_id}", user.id);
    put(&format!("{prefix}/{index}"), data, None).await?;
    if index < total - 1 {
        return Ok(None);
    }
    let limit = max_mb(db, kind).await? * 1024 * 1024;
    let mut all: Vec<u8> = Vec::new();
    let mut failed: Option<ApiError> = None;
    for i in 0..total {
        let part = if i == index {
            data.to_vec()
        } else {
            match read(&format!("{prefix}/{i}")).await {
                Ok(p) => p,
                Err(_) => {
                    failed = Some(bad_part(
                        "A piece of the upload is missing; upload the file again",
                    ));
                    break;
                }
            }
        };
        all.extend_from_slice(&part);
        if all.len() as i64 > limit {
            failed = Some(ApiError::http(
                413,
                format!(
                    "The {} is too large ({} MB at most)",
                    if kind == "recording" {
                        "recording"
                    } else {
                        "notes file"
                    },
                    limit / (1024 * 1024)
                ),
            ));
            break;
        }
    }
    let store = minio();
    for i in 0..total {
        // a stray piece must not fail the upload
        let _ = store
            .remove_object(&bucket(), &format!("{prefix}/{i}"))
            .await;
    }
    if let Some(e) = failed {
        return Err(e);
    }
    save_upload(db, user, meeting, filename, mime, &all, source)
        .await
        .map(Some)
}

/// A file the Owner (anyone holding nary:approve) sent Nary on WhatsApp: when it is notes or a recording it is
/// kept (up to the recording limit) and registered, waiting for its meeting. None for other files, or a
/// file that could not be fetched (`media_status` says why).
pub async fn from_whatsapp(
    db: &mut PgConnection,
    user: &AuthUser,
    chat: &WaChat,
    msg: &mut WaMessage,
) -> ApiResult<Option<MeetingFile>> {
    if !msg.has_media {
        return Ok(None);
    }
    let name = msg
        .media_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            let ext = crate::nary::media::guess_extension(
                msg.media_mime
                    .as_deref()
                    .unwrap_or("")
                    .split(';')
                    .next()
                    .unwrap_or(""),
            )
            .unwrap_or("");
            format!(
                "whatsapp-{}{ext}",
                msg.message_key.chars().take(12).collect::<String>()
            )
        });
    let Some(kind) = kind_of(&name, msg.media_mime.as_deref()) else {
        return Ok(None);
    };
    let existing: Option<MeetingFile> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM nary_meeting_files WHERE wa_message_key = $1"
    ))
    .bind(&msg.message_key)
    .fetch_optional(&mut *db)
    .await?;
    if existing.is_some() {
        return Ok(existing);
    }
    let mb = max_mb(db, kind).await?;
    if !crate::nary::media::store_up_to(db, chat, msg, mb).await? {
        return Ok(None);
    }
    let row: Option<MeetingFile> = sqlx::query_as(&format!(
        "INSERT INTO nary_meeting_files (id, kind, filename, mime, size_bytes, object_key, source, wa_message_key, \
         status, requested_by_id) VALUES ($1, $2, $3, $4, $5, $6, 'whatsapp', $7, 'stored', $8) \
         ON CONFLICT (wa_message_key) WHERE wa_message_key IS NOT NULL DO NOTHING RETURNING {COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(kind)
    .bind(name.chars().take(255).collect::<String>())
    .bind(&msg.media_mime)
    .bind(i64::from(msg.media_size.unwrap_or(0)))
    .bind(msg.media_key.clone().unwrap_or_default())
    .bind(&msg.message_key)
    .bind(user.id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row)
}

/// The line the assistant reads for a file sent in a chat (it names the file_id for attach_meeting_notes).
pub fn chat_line(f: &MeetingFile) -> String {
    let what = if f.kind == "recording" {
        "Meeting recording"
    } else {
        "Meeting notes file"
    };
    // batch 4 D1: a notes file is a file like any other too: read_attachment reads it with the same id
    let read = if f.kind == "notes" {
        " If it is not meeting notes, read_attachment reads it."
    } else {
        ""
    };
    format!(
        "[{what} attached: {} (file_id {}). Ask which meeting it is for unless it is clear, then call attach_meeting_notes.{read}]",
        f.filename, f.id
    )
}

// ─── Which meeting ────────────────────────────────────────────────────────────

/// A past meeting as the tool lists it.
async fn candidates(db: &mut PgConnection, term: Option<&str>) -> ApiResult<Vec<Value>> {
    let rows: Vec<(Uuid, Option<String>, Option<DateTime<Utc>>, String, Option<Uuid>, Option<String>)> =
        sqlx::query_as(
            "SELECT m.id, m.title, m.starts_at, m.status, m.deal_id, \
                    COALESCE(o.name, NULLIF(trim(c.first_name || ' ' || COALESCE(c.last_name, '')), ''), \
                             m.attendee_email) \
             FROM nary_meetings m \
             LEFT JOIN contacts c ON c.id = m.contact_id \
             LEFT JOIN deals d ON d.id = m.deal_id \
             LEFT JOIN leads l ON l.id = m.lead_id \
             LEFT JOIN organizations o ON o.id = COALESCE(d.organization_id, l.organization_id, c.organization_id) \
             WHERE m.status IN ('booked', 'done') AND (m.starts_at IS NULL OR m.starts_at <= now()) \
               AND ($1::text IS NULL OR m.title ILIKE $1 OR o.name ILIKE $1 OR d.title ILIKE $1 OR l.title ILIKE $1 \
                    OR (c.first_name || ' ' || COALESCE(c.last_name, '')) ILIKE $1 OR m.attendee_email ILIKE $1) \
             ORDER BY m.starts_at DESC NULLS LAST, m.created_at DESC LIMIT 5",
        )
        .bind(term.map(|t| format!("%{}%", t.trim())))
        .fetch_all(&mut *db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, title, starts, status, deal, customer)| {
            json!({"meeting_id": id.to_string(), "title": title, "starts_at": iso(starts.as_ref()),
                   "status": status, "deal_id": deal.map(|d| d.to_string()), "customer": customer})
        })
        .collect())
}

/// The meeting a file is for: the one named, else the latest past meeting with that customer. Err: what
/// to tell the model (with the latest past meetings to pick from).
pub async fn resolve_meeting(
    db: &mut PgConnection,
    meeting_id: Option<Uuid>,
    customer: Option<&str>,
) -> ApiResult<Result<NaryMeeting, Value>> {
    if let Some(id) = meeting_id {
        return Ok(match load_meeting(db, id).await? {
            Some(m) => Ok(m),
            None => Err(json!({"error": "No meeting has that meeting_id.",
                               "meetings": candidates(db, None).await?})),
        });
    }
    if let Some(term) = customer.map(str::trim).filter(|t| !t.is_empty()) {
        let found = candidates(db, Some(term)).await?;
        if let Some(id) = found
            .first()
            .and_then(|m| m["meeting_id"].as_str())
            .and_then(|s| s.parse::<Uuid>().ok())
        {
            if let Some(m) = load_meeting(db, id).await? {
                return Ok(Ok(m));
            }
        }
        return Ok(Err(json!({
            "error": format!("No past meeting with {term} was found. Ask the user which meeting it is."),
            "meetings": candidates(db, None).await?})));
    }
    Ok(Err(json!({
        "error": "Say which meeting the file is for: meeting_id, or the customer (their latest past meeting). Ask the user if it is not clear.",
        "meetings": candidates(db, None).await?})))
}

/// Give a stored file its meeting and queue it (a failed one may be tried again). The tick reads it.
pub async fn attach(
    db: &mut PgConnection,
    file: &MeetingFile,
    meeting: &NaryMeeting,
) -> ApiResult<MeetingFile> {
    let row: MeetingFile = sqlx::query_as(&format!(
        "UPDATE nary_meeting_files SET meeting_id = $2, deal_id = $3, status = 'queued', progress = 0, error = NULL, \
         stt_job_id = NULL, next_poll_at = now(), finished_at = NULL, updated_at = now() WHERE id = $1 \
         RETURNING {COLS}"
    ))
    .bind(file.id)
    .bind(meeting.id)
    .bind(meeting.deal_id)
    .fetch_one(&mut *db)
    .await?;
    Ok(row)
}

fn holds(user: &AuthUser, code: &str) -> bool {
    user.is_superadmin || user.perms.contains(code)
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// The tool `attach_meeting_notes` (catalog): a notes file or a recording sent in the chat goes to its
/// meeting and is read like an uploaded transcript (direct, no card: the same people who may upload a
/// transcript on Nary → Meetings; what the debrief changes on the deal still comes as cards).
pub async fn _h_attach_meeting_notes(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !holds(user, "nary:approve") {
        return Ok(
            json!({"error": "Only people who may give Nary a meeting transcript (nary:approve) can do this."}),
        );
    }
    let Some(file_id) = arg_str(&args, "file_id").and_then(|s| s.parse::<Uuid>().ok()) else {
        return Ok(json!({"error": "Give the file_id from the attached file's line in the chat."}));
    };
    let Some(file) = load(db, file_id).await? else {
        return Ok(json!({"error": "No meeting file has that file_id."}));
    };
    let is_approver = user.is_superadmin || roles::approver_ids(db).await?.contains(&user.id);
    if file.requested_by_id != Some(user.id) && !is_approver {
        return Ok(json!({"error": "That file was sent by someone else."}));
    }
    if WORKING.contains(&file.status.as_str()) {
        return Ok(json!({"note": "Nary is already reading this file.", "file": view(&file)}));
    }
    if file.status == "done" {
        return Ok(
            json!({"note": "This file was already read; its results are on the deal.", "file": view(&file)}),
        );
    }
    let meeting_id = match arg_str(&args, "meeting_id") {
        Some(raw) => match raw.parse::<Uuid>() {
            Ok(id) => Some(id),
            Err(_) => return Ok(json!({"error": "meeting_id is not a valid id."})),
        },
        None => None,
    };
    let customer = arg_str(&args, "customer");
    let meeting = match resolve_meeting(db, meeting_id, customer.as_deref()).await? {
        Ok(m) => m,
        Err(why) => return Ok(why),
    };
    let row = attach(db, &file, &meeting).await?;
    let note = if row.kind == "recording" {
        "Queued: Nary transcribes the recording on the server (a long one can take a while), then reads it. \
         The results go on the deal and the Owner gets a WhatsApp message when they are there."
    } else {
        "Queued: Nary reads the notes within a minute. The results go on the deal (results note, follow-ups, a \
         card for any stage or value change, the summary email draft) and the Owner gets a WhatsApp message."
    };
    Ok(json!({
        "done": true, "file": view(&row),
        "meeting": {"meeting_id": meeting.id.to_string(), "title": meeting.title,
                    "starts_at": iso(meeting.starts_at.as_ref()),
                    "deal_id": meeting.deal_id.map(|d| d.to_string())},
        "note": note,
    }))
}

// ─── The worker ───────────────────────────────────────────────────────────────

async fn set_progress(db: &mut PgConnection, id: Uuid, progress: i32) -> ApiResult<()> {
    sqlx::query(
        "UPDATE nary_meeting_files SET progress = GREATEST(progress, $2), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(progress.clamp(0, 99))
    .execute(&mut *db)
    .await?;
    Ok(())
}

fn detail_text(e: &ApiError) -> String {
    match e {
        ApiError::Http { detail, .. } => match detail {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        other => {
            tracing::warn!(error = %other, "nary_meeting_file_error");
            "an internal error (see the server log)".to_string()
        }
    }
}

/// What a meeting is called in a message.
fn meeting_label(m: Option<&NaryMeeting>) -> String {
    m.and_then(|m| m.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "the meeting".to_string())
}

async fn fail(db: &mut PgConnection, f: &MeetingFile, reason: &str) -> ApiResult<()> {
    sqlx::query(
        "UPDATE nary_meeting_files SET status = 'failed', error = $2, finished_at = now(), next_poll_at = NULL, \
         updated_at = now() WHERE id = $1",
    )
    .bind(f.id)
    .bind(reason)
    .execute(&mut *db)
    .await?;
    let meeting = match f.meeting_id {
        Some(id) => load_meeting(db, id).await?,
        None => None,
    };
    let what = if f.kind == "recording" {
        "the recording"
    } else {
        "the notes"
    };
    let title = format!("Couldn't read {what}: {}", meeting_label(meeting.as_ref()));
    let body = format!(
        "Nary couldn't read {what} {} for {}: {reason}. Upload it again on Nary → Meetings or send the transcript.",
        f.filename,
        meeting_label(meeting.as_ref())
    );
    let eid = f.meeting_id.map(|m| m.to_string()).unwrap_or_default();
    let mut t = owner_channel::Tell::new(
        &title,
        &body,
        "/nary/meetings",
        "nary_meeting",
        &eid,
        "nary_urgent",
    );
    t.urgent_kind = Some("failure");
    owner_channel::tell(db, t).await?;
    commit(db).await
}

/// The debrief with the file's text, then the Owner's message. The file is claimed ('analysing') first.
async fn analyse(db: &mut PgConnection, f: &MeetingFile) -> ApiResult<&'static str> {
    let claimed = sqlx::query(
        "UPDATE nary_meeting_files SET status = 'analysing', updated_at = now() \
         WHERE id = $1 AND status IN ('queued', 'transcribing')",
    )
    .bind(f.id)
    .execute(&mut *db)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok("busy");
    }
    commit(db).await?;
    let Some(meeting) = (match f.meeting_id {
        Some(id) => load_meeting(db, id).await?,
        None => None,
    }) else {
        fail(db, f, "its meeting is gone").await?;
        return Ok("failed");
    };
    let text = if f.kind == "notes" {
        let data = match read(&f.object_key).await {
            Ok(d) => d,
            Err(_) => {
                fail(db, f, "the stored file could not be read").await?;
                return Ok("failed");
            }
        };
        match meetings::transcript_text(&notes_name(f), &data) {
            Ok(t) => t,
            Err(_) => {
                fail(
                    db,
                    f,
                    "the file is not a readable text, docx, SRT or VTT file",
                )
                .await?;
                return Ok("failed");
            }
        }
    } else {
        f.transcript.clone().unwrap_or_default()
    };
    if text.trim().chars().count() < 20 {
        let why = if f.kind == "recording" {
            "no speech was heard in it"
        } else {
            "it is empty"
        };
        fail(db, f, why).await?;
        return Ok("failed");
    }
    if f.kind == "notes" {
        sqlx::query(
            "UPDATE nary_meeting_files SET transcript = $2, updated_at = now() WHERE id = $1",
        )
        .bind(f.id)
        .bind(&text)
        .execute(&mut *db)
        .await?;
        commit(db).await?;
    }
    let actor = match f.requested_by_id {
        Some(id) => roles::load_actor(db, id).await?,
        None => None,
    };
    let actor = match actor {
        Some(a) => a,
        None => match roles::approver_ids(db).await?.first() {
            Some(id) => match roles::load_actor(db, *id).await? {
                Some(a) => a,
                None => {
                    fail(db, f, "nobody can act for it (the uploader is gone)").await?;
                    return Ok("failed");
                }
            },
            None => {
                fail(db, f, "nobody can act for it (the uploader is gone)").await?;
                return Ok("failed");
            }
        },
    };
    let source = if f.kind == "recording" {
        format!("meeting recording ({})", f.filename)
    } else {
        format!("meeting notes ({})", f.filename)
    };
    let source: String = source.chars().take(80).collect();
    let out = match meetings::debrief(db, Some(&meeting), None, &text, &source, &actor).await {
        Ok(v) => v,
        Err(e) => {
            rollback(db).await?;
            let why = detail_text(&e);
            fail(db, f, &why).await?;
            return Ok("failed");
        }
    };
    // the meeting may have been linked to the deal by the debrief
    let meeting = load_meeting(db, meeting.id).await?.unwrap_or(meeting);
    sqlx::query(
        "UPDATE nary_meeting_files SET status = 'done', progress = 100, result = $2, deal_id = COALESCE($3, deal_id), \
         finished_at = now(), next_poll_at = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(f.id)
    .bind(&out)
    .bind(meeting.deal_id)
    .execute(&mut *db)
    .await?;
    tell_done(db, f, &meeting, &out).await?;
    commit(db).await?;
    Ok("done")
}

/// One message for the Owner: where the results are, and that a stage/value change waits as a card (the Owner
/// channel bundles it with that card's approval when both are raised together).
async fn tell_done(
    db: &mut PgConnection,
    f: &MeetingFile,
    meeting: &NaryMeeting,
    out: &Value,
) -> ApiResult<()> {
    let deal: Option<(Uuid, String)> = match meeting.deal_id {
        Some(id) => {
            sqlx::query_as("SELECT id, title FROM deals WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let what = if f.kind == "recording" {
        "recording"
    } else {
        "notes"
    };
    let summary: String = out
        .get("extract")
        .and_then(|e| e.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(400)
        .collect();
    let commitments = out.get("commitments").and_then(Value::as_i64).unwrap_or(0);
    let mut lines = vec![];
    if !summary.is_empty() {
        lines.push(summary);
    }
    let place = match &deal {
        Some((_, t)) => format!("on the deal {t}"),
        None => "on the lead".to_string(),
    };
    lines.push(format!(
        "I read the {what} of {} ({}): the results are {place}, with {commitments} follow-up(s) for us.",
        meeting_label(Some(meeting)),
        f.filename
    ));
    if out.get("deal_update_card").is_some_and(|c| c.is_string()) {
        lines.push(
            "The deal's stage or value changed in the call: the change waits for your approval."
                .into(),
        );
    }
    if out.get("summary_card").is_some_and(|c| c.is_string()) {
        lines.push("The summary email to the customer is drafted for your approval.".into());
    }
    let body = lines.join("\n");
    let title = format!("Meeting results: {}", meeting_label(Some(meeting)));
    let (link, etype, eid) = match &deal {
        Some((id, _)) => (format!("/crm/deals/{id}"), "deal", id.to_string()),
        None => (
            "/nary/meetings".to_string(),
            "nary_meeting",
            meeting.id.to_string(),
        ),
    };
    let t = owner_channel::Tell::new(&title, &body, &link, etype, &eid, "nary_done");
    owner_channel::tell(db, t).await?;
    Ok(())
}

/// Send a queued recording to the speech-to-text job API.
async fn submit(db: &mut PgConnection, f: &MeetingFile) -> ApiResult<&'static str> {
    let minutes = require_int(db, "nary.meetings.recording_max_minutes", WHERE).await?;
    if f.duration_seconds
        .is_some_and(|d| d > (minutes * 60) as f64)
    {
        fail(
            db,
            f,
            &format!("the recording is longer than {minutes} minutes"),
        )
        .await?;
        return Ok("failed");
    }
    let language = match f.meeting_id {
        Some(id) => load_meeting(db, id)
            .await?
            .and_then(|m| m.language)
            .filter(|l| !l.is_empty()),
        None => None,
    };
    let data = match read(&f.object_key).await {
        Ok(d) => d,
        Err(_) => {
            fail(db, f, "the stored file could not be read").await?;
            return Ok("failed");
        }
    };
    let mime = f
        .mime
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "application/octet-stream".into());
    match stt_client::submit_job(
        data,
        &f.filename,
        &mime,
        language.as_deref(),
        Some(minutes * 60),
    )
    .await
    {
        Ok(job_id) => {
            sqlx::query(
                "UPDATE nary_meeting_files SET stt_job_id = $2, status = 'transcribing', submits = submits + 1, \
                 language = COALESCE(language, $3), started_at = COALESCE(started_at, now()), updated_at = now() \
                 WHERE id = $1",
            )
            .bind(f.id)
            .bind(&job_id)
            .bind(&language)
            .execute(&mut *db)
            .await?;
            commit(db).await?;
            Ok("submitted")
        }
        Err(JobError::Refused(why)) => {
            fail(db, f, &why).await?;
            Ok("failed")
        }
        Err(e) => {
            tracing::warn!(file = %f.id, error = %e, "nary_recording_submit_later");
            Ok("waiting")
        }
    }
}

/// Ask how a recording is going; when its transcript is in, read it.
async fn poll(db: &mut PgConnection, f: &MeetingFile, job_id: &str) -> ApiResult<&'static str> {
    match stt_client::job_status(job_id).await {
        Ok(st) => match st.status.as_str() {
            "done" => {
                let minutes = require_int(db, "nary.meetings.recording_max_minutes", WHERE).await?;
                if st.duration.is_some_and(|d| d > (minutes * 60) as f64 + 1.0) {
                    fail(
                        db,
                        f,
                        &format!("the recording is longer than {minutes} minutes"),
                    )
                    .await?;
                    return Ok("failed");
                }
                let text = st.text.clone().unwrap_or_default();
                sqlx::query(
                    "UPDATE nary_meeting_files SET transcript = $2, language = COALESCE($3, language), \
                     duration_seconds = COALESCE($4, duration_seconds), progress = 100, updated_at = now() \
                     WHERE id = $1",
                )
                .bind(f.id)
                .bind(&text)
                .bind(&st.language)
                .bind(st.duration)
                .execute(&mut *db)
                .await?;
                commit(db).await?;
                let mut fresh = f.clone();
                fresh.transcript = Some(text);
                analyse(db, &fresh).await
            }
            "failed" => {
                let why = st
                    .error
                    .unwrap_or_else(|| "speech-to-text could not read it".into());
                fail(db, f, &why).await?;
                Ok("failed")
            }
            _ => {
                set_progress(db, f.id, st.progress).await?;
                commit(db).await?;
                Ok("transcribing")
            }
        },
        Err(JobError::Lost) => {
            let again = require_int(db, "nary.meetings.recording_resubmits", WHERE).await?;
            if i64::from(f.submits) > again {
                fail(
                    db,
                    f,
                    "speech-to-text lost it (the service restarted) too many times",
                )
                .await?;
                return Ok("failed");
            }
            sqlx::query(
                "UPDATE nary_meeting_files SET status = 'queued', stt_job_id = NULL, progress = 0, \
                 next_poll_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(f.id)
            .execute(&mut *db)
            .await?;
            commit(db).await?;
            Ok("resubmit")
        }
        Err(e) => {
            tracing::warn!(file = %f.id, error = %e, "nary_recording_poll_later");
            Ok("waiting")
        }
    }
}

/// The minute tick: every file due (queued, or transcribing and due for a poll) moves one step. Returns
/// what happened per file ({} when nothing was due).
pub async fn tick(db: &mut PgConnection) -> ApiResult<Value> {
    let poll_s = require_int(db, "nary.meetings.recording_poll_seconds", WHERE).await?;
    let deadline_h = require_int(db, "nary.meetings.recording_deadline_hours", WHERE).await?;
    let due: Vec<MeetingFile> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM nary_meeting_files WHERE status IN ('queued', 'transcribing') \
         AND (next_poll_at IS NULL OR next_poll_at <= now()) ORDER BY created_at LIMIT 20"
    ))
    .fetch_all(&mut *db)
    .await?;
    let mut done = Map::new();
    for f in due {
        // claim this round (a slow debrief must not let the next tick run the same file)
        let claimed = sqlx::query(
            "UPDATE nary_meeting_files SET next_poll_at = now() + make_interval(secs => $2), updated_at = now() \
             WHERE id = $1 AND status = $3 AND (next_poll_at IS NULL OR next_poll_at <= now())",
        )
        .bind(f.id)
        .bind(poll_s as f64)
        .bind(&f.status)
        .execute(&mut *db)
        .await?
        .rows_affected();
        commit(db).await?;
        if claimed == 0 {
            continue;
        }
        let outcome = if f.created_at + Duration::hours(deadline_h) < Utc::now() {
            fail(
                db,
                &f,
                &format!("it was not transcribed within {deadline_h} hours"),
            )
            .await?;
            "failed"
        } else if f.kind == "notes" {
            analyse(db, &f).await?
        } else {
            match f.stt_job_id.clone() {
                None => submit(db, &f).await?,
                Some(job) => poll(db, &f, &job).await?,
            }
        };
        done.insert(f.id.to_string(), json!(outcome));
    }
    Ok(Value::Object(done))
}

/// The transcript to download: (file name, text).
pub async fn transcript(db: &mut PgConnection, id: Uuid) -> ApiResult<(String, String)> {
    let Some(f) = load(db, id).await? else {
        return Err(ApiError::http(404, "File not found"));
    };
    let Some(text) = f.transcript.filter(|t| !t.is_empty()) else {
        return Err(ApiError::http(
            404,
            "There is no transcript for this file yet",
        ));
    };
    let stem = match f.filename.rfind('.') {
        Some(i) if i > 0 => &f.filename[..i],
        _ => f.filename.as_str(),
    };
    Ok((format!("{stem}-transcript.txt"), text))
}

// ─── On request (3 Nov 2026) ──────────────────────────────────────────────────

/// 3 Nov 2026 (the MCP client as Nary, Stream B2): `transcribe_meeting_file` runs one file now, step after step,
/// as the minute tick would (notes read at once; a recording submitted to speech-to-text, polled every
/// nary.meetings.recording_poll_seconds, then the debrief), until it is done or failed or its deadline passes.
/// The tick is part of Nary's playbooks, which wait while Nary is paused; this runs on request (the caller sets
/// the request scope, executor::on_request, so the debrief's cards are raised while paused). Each step claims
/// the file like the tick does, so a running tick and this never run the same step. Returns the file's view with
/// its result and what the last step did.
pub async fn process_on_request(db: &mut PgConnection, id: Uuid) -> ApiResult<Value> {
    let poll_s = require_int(db, "nary.meetings.recording_poll_seconds", WHERE).await?;
    let deadline_h = require_int(db, "nary.meetings.recording_deadline_hours", WHERE).await?;
    let pause = std::time::Duration::from_secs(poll_s.max(1) as u64);
    let mut last = "waiting";
    loop {
        let Some(f) = load(db, id).await? else {
            return Err(ApiError::http(404, "Meeting file not found"));
        };
        if matches!(f.status.as_str(), "done" | "failed" | "stored") {
            let mut out = view(&f);
            out["step"] = json!(last);
            out["result"] = f.result.clone().unwrap_or(Value::Null);
            return Ok(out);
        }
        // claim this step (a tick that runs meanwhile skips it, as two ticks do)
        let claimed = if f.status == "analysing" {
            0
        } else {
            sqlx::query(
                "UPDATE nary_meeting_files SET next_poll_at = now() + make_interval(secs => $2), updated_at = now() \
                 WHERE id = $1 AND status = $3 AND (next_poll_at IS NULL OR next_poll_at <= now())",
            )
            .bind(f.id)
            .bind(poll_s as f64)
            .bind(&f.status)
            .execute(&mut *db)
            .await?
            .rows_affected()
        };
        commit(db).await?;
        if claimed > 0 {
            last = if f.created_at + Duration::hours(deadline_h) < Utc::now() {
                fail(
                    db,
                    &f,
                    &format!("it was not transcribed within {deadline_h} hours"),
                )
                .await?;
                "failed"
            } else if f.kind == "notes" {
                analyse(db, &f).await?
            } else {
                match f.stt_job_id.clone() {
                    None => submit(db, &f).await?,
                    Some(job) => poll(db, &f, &job).await?,
                }
            };
            if matches!(last, "done" | "failed") {
                continue;
            }
        }
        tokio::time::sleep(pause).await;
    }
}
