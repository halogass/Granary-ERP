//! backend/app/routers/nary_playbooks.py, mounted at `/api/v1/nary` (backend/app/main.py).
//!
//! Nary's sales playbooks in the ERP (Phase 3): Google Calendar connection, meetings and debriefs, the
//! Nary panel of a deal or lead, the weekly report and SOP revision drafts.
//!
//! `router` is mounted with `dependencies=INTERNAL_ONLY` (the registry wraps it); `public_router` is not:
//! Google's OAuth redirect carries no ERP session, the one-time state ties it to the request.
//!
//! Permissions are checked in the body (`_require` from nary.py), after the body was validated.
//! Python quirk kept: `uuid.UUID(meeting_id)` / `uuid.UUID(deal_id)` / `uuid.UUID(lead_id)` on a malformed
//! id raises ValueError, which the app turns into the 500 handler's response (not a 400 or 404).

use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use granary_compat::pyjson::{PyDateTime, PyTz};
use granary_compat::text::pystr;
use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::{Cx, DateTimeIn, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm::{Contact, Deal};
use granary_db::rows::nary::{NaryInsight, NaryMeeting, NaryScore, WaChat};
use granary_services::nary::governance::{http_detail, is_http_exception};
use granary_services::nary::{
    cadence, channels, meeting_files, meetings, replies, report, sop_revisions,
};

use super::at;
use super::nary::{_holds, _iso, _require};
use crate::extract::{PyForm, UploadFile};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/nary";

/// `MAX_UPLOAD = 40 * 1024 * 1024`.
pub const MAX_UPLOAD: usize = 40 * 1024 * 1024;

/// The columns of nary_meetings, as services/nary/meetings.rs reads them.
const MEETING_COLS: &str = "deal_id, lead_id, contact_id, chat_id, channel, language, customer_tz, country, slots, \
    status, picked, starts_at, ends_at, event_id, meet_link, attendee_email, title, day_reminder_at, \
    hour_reminder_at, brief_at, card_id, notes, origin, booked_by_id, attendees, id, created_at, updated_at";

/// The routes of `router` in nary_playbooks.py, in its order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/google"), get(google_status))
        .route(&at(PREFIX, "/google/credentials"), put(google_credentials))
        .route(&at(PREFIX, "/google/connect"), post(google_connect))
        .route(&at(PREFIX, "/meetings/book"), post(meeting_book))
        .route(&at(PREFIX, "/meetings"), get(meetings_list))
        .route(
            &at(PREFIX, "/meetings/{meeting_id}/debrief"),
            post(meeting_debrief),
        )
        .route(&at(PREFIX, "/debrief/text"), post(debrief_text))
        // batch 3 R1: meeting recordings and notes files
        .route(
            &at(PREFIX, "/meetings/{meeting_id}/recording"),
            post(meeting_recording),
        )
        .route(
            &at(PREFIX, "/meeting-files"),
            get(meeting_files_list).post(meeting_file_upload),
        )
        .route(
            &at(PREFIX, "/meeting-files/{file_id}/transcript"),
            get(meeting_file_transcript),
        )
        // batch 4 D1: any file attached in the web chat (read with read_attachment)
        .route(&at(PREFIX, "/chat-files"), post(chat_file_upload))
        .route(&at(PREFIX, "/deals/{deal_id}/debrief"), post(deal_debrief))
        .route(
            &at(PREFIX, "/meetings/{meeting_id}/no-show"),
            post(meeting_no_show),
        )
        .route(&at(PREFIX, "/deals/{deal_id}/panel"), get(deal_panel))
        .route(&at(PREFIX, "/leads/{lead_id}/panel"), get(lead_panel))
        .route(&at(PREFIX, "/deals/{deal_id}/meeting"), post(deal_meeting))
        .route(&at(PREFIX, "/report"), get(report_view))
        .route(&at(PREFIX, "/report/send"), post(report_send))
        .route(
            &at(PREFIX, "/sop-revisions"),
            get(sop_briefs).post(sop_draft),
        )
}

/// `nary_playbooks.public_router`, mounted at /api/v1/nary WITHOUT INTERNAL_ONLY: Google's OAuth redirect
/// carries no ERP session; the one-time state ties it to the request.
pub fn public_router() -> Router<AppState> {
    Router::new().route(&at(PREFIX, "/google/callback"), get(google_callback))
}

// ─── helpers ──────────────────────────────────────────────────────────────────

fn internal(msg: &str) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

/// `uuid.UUID(s)` on a router line: a ValueError is the central 422 "Invalid id format".
fn py_uuid(s: &str) -> ApiResult<Uuid> {
    crate::extract::router_uuid(s)
}

/// `urllib.parse.quote(s, safe)`: percent-encodes every UTF-8 byte outside the always-safe set and `safe`.
fn py_quote(s: &str, safe: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        let c = b as char;
        if b.is_ascii_alphanumeric() || "_.-~".contains(c) || (b.is_ascii() && safe.contains(c)) {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `RedirectResponse(url)`: 307, `location` quoted with Starlette's safe set, empty body.
fn redirect(url: &str) -> Response {
    let location = py_quote(url, ":/%#?=@[]!$&'()*+,;");
    let mut r = (StatusCode::TEMPORARY_REDIRECT, "").into_response();
    r.headers_mut().remove("content-type");
    if let Ok(v) = HeaderValue::from_str(&location) {
        r.headers_mut().insert("location", v);
    }
    r
}

/// Pydantic's datetime as the services' PyDateTime (an offset of 0 is pydantic's `TzInfo(UTC)`).
fn py_datetime(d: &DateTimeIn) -> PyDateTime {
    PyDateTime {
        local: d.naive,
        tz: match d.offset {
            None => None,
            Some(0) => Some(PyTz::Utc),
            Some(o) => Some(PyTz::Fixed(o)),
        },
    }
}

/// Python truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
    }
}

/// `_char_slice(s, n)`: `s[:n]` (characters).
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn load_meeting(db: &mut sqlx::PgConnection, id: Uuid) -> ApiResult<Option<NaryMeeting>> {
    Ok(sqlx::query_as::<_, NaryMeeting>(&format!(
        "SELECT {MEETING_COLS} FROM nary_meetings WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn load_deal(db: &mut sqlx::PgConnection, id: Uuid) -> ApiResult<Option<Deal>> {
    Ok(
        sqlx::query_as::<_, Deal>("SELECT * FROM deals WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

// ─── Google Calendar ──────────────────────────────────────────────────────────

/// `GET /google`.
pub async fn google_status(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    Ok(resp::json(
        granary_services::google_calendar::status(&mut conn).await?,
    ))
}

granary_core::py_model! {
    /// `class GoogleCredentials(BaseModel)`.
    pub struct GoogleCredentials {
        pub client_id: String,
        /// blank keeps the saved one
        pub client_secret: Option<String> = None,
    }
}

/// `PUT /google/credentials`.
pub async fn google_credentials(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<GoogleCredentials>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, Option<String>, Option<bool>, Option<Uuid>)> = sqlx::query_as(
        "SELECT key, value, is_encrypted, updated_by_id FROM system_settings \
         WHERE key IN ('nary.google.client_id', 'nary.google.client_secret')",
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() != 2 {
        return Err(ApiError::http(
            400,
            "Run the database migrations first (nary.google.* missing)",
        ));
    }
    let secret = pystr::strip(body.client_secret.as_deref().unwrap_or("")).to_string();
    for (key, value, is_encrypted, updated_by) in rows {
        // The ORM flushes an UPDATE (with updated_at's onupdate) only when an attribute changed.
        let (mut new_value, mut new_enc) = (value.clone(), is_encrypted);
        if key == "nary.google.client_id" {
            new_value = Some(pystr::strip(&body.client_id).to_string());
        } else if !secret.is_empty() {
            new_value = Some(granary_services::email_crypto::encrypt(&secret));
            new_enc = Some(true);
        }
        if new_value != value || new_enc != is_encrypted || updated_by != Some(user.id) {
            sqlx::query(
                "UPDATE system_settings SET value = $2, is_encrypted = $3, updated_by_id = $4, \
                 updated_at = now() WHERE key = $1",
            )
            .bind(&key)
            .bind(&new_value)
            .bind(new_enc)
            .bind(user.id)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    Ok(resp::json(
        granary_services::google_calendar::status(&mut conn).await?,
    ))
}

/// `POST /google/connect`.
pub async fn google_connect(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let url = granary_services::google_calendar::start_connect(&mut conn, &user).await?;
    Ok(resp::json(json!({ "url": url })))
}

granary_core::py_model! {
    /// The query of `google_callback`: `code: str = "", state: str = "", error: str = ""`.
    pub struct CallbackQ {
        pub code: String = String::new(),
        pub state: String = String::new(),
        pub error: String = String::new(),
    }
}

/// `GET /google/callback` (public_router): Google's redirect back; always redirects to the Meetings page.
pub async fn google_callback(db: Db, PyQuery(q): PyQuery<CallbackQ>) -> ApiResult<Response> {
    let app_url = granary_core::config::settings().APP_URL.clone();
    let target = format!("{}/nary/meetings", app_url.trim_end_matches('/'));
    if !q.error.is_empty() || q.code.is_empty() {
        let reason = if q.error.is_empty() {
            "no_code"
        } else {
            &q.error
        };
        return Ok(redirect(&format!("{target}?google=error&reason={reason}")));
    }
    let mut conn = db.conn().await?;
    match granary_services::google_calendar::finish_connect(&mut conn, &q.code, &q.state).await {
        Ok(_) => {}
        Err(e) if is_http_exception(&e) => {
            let detail = http_detail(&e).unwrap_or_default();
            return Ok(redirect(&format!(
                "{target}?google=error&reason={}",
                py_quote(&head(&detail, 200), "/")
            )));
        }
        Err(e) => return Err(e),
    }
    Ok(redirect(&format!("{target}?google=connected")))
}

// ─── Meetings ─────────────────────────────────────────────────────────────────

/// `_meeting(m)`.
pub fn _meeting(m: &NaryMeeting) -> Value {
    json!({
        "id": m.id.to_string(), "status": m.status, "title": m.title, "chat_id": m.chat_id, "channel": m.channel,
        "deal_id": m.deal_id.map(|v| v.to_string()), "lead_id": m.lead_id.map(|v| v.to_string()),
        "slots": m.slots, "picked": m.picked, "starts_at": _iso(m.starts_at.as_ref()),
        "ends_at": _iso(m.ends_at.as_ref()),
        "meet_link": m.meet_link, "attendee_email": m.attendee_email, "customer_tz": m.customer_tz,
        "card_id": m.card_id.map(|v| v.to_string()), "created_at": _iso(Some(&m.created_at)),
        "origin": m.origin, "attendees": if truthy(Some(&m.attendees)) { m.attendees.clone() } else { json!([]) },
    })
}

granary_core::py_model! {
    /// `class BookMeeting(BaseModel)`.
    pub struct BookMeeting {
        pub title: String,
        pub start: DateTimeIn,
        pub duration_minutes: Option<i64> = None,
        pub staff_ids: Vec<Uuid> = Vec::new(),
        pub customer_email: Option<String> = None,
        pub lead_id: Option<Uuid> = None,
        pub deal_id: Option<Uuid> = None,
        pub contact_id: Option<Uuid> = None,
        /// {channel, text (with {link}), chat_id | thread_id | contact_id | to, subject}
        pub message: Option<Map<String, Value>> = None,
    }
}

/// `POST /meetings/book`: book a meeting at the given time (Nary's meeting card).
pub async fn meeting_book(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<BookMeeting>,
) -> ApiResult<Response> {
    if !["nary:approve", "crm:edit_deal", "crm:edit_lead"]
        .iter()
        .any(|c| _holds(&user, c))
    {
        return Err(ApiError::http(
            403,
            "Permission required: nary:approve, crm:edit_deal or crm:edit_lead",
        ));
    }
    let message = body.message.as_ref().filter(|m| !m.is_empty());
    if message.is_some() && !_holds(&user, "nary:approve") {
        return Err(ApiError::http(
            403,
            "Sending the customer the link as Nary needs nary:approve; book without the message and send it yourself",
        ));
    }
    if let Some(m) = message {
        let channel_ok =
            matches!(m.get("channel"), Some(Value::String(c)) if c == "email" || c == "whatsapp");
        // `(message.get("text") or "").strip()`: a truthy non-string has no .strip() (AttributeError).
        let text_empty = match m.get("text") {
            Some(Value::String(s)) => pystr::strip(s).is_empty(),
            v if !truthy(v) => true,
            _ => return Err(internal("'text' has no attribute 'strip'")),
        };
        if !channel_ok || text_empty {
            return Err(ApiError::http(
                400,
                "The message needs a channel (email or whatsapp) and its text",
            ));
        }
    }
    let title = pystr::strip(&body.title);
    if title.is_empty() {
        return Err(ApiError::http(400, "The meeting needs a title"));
    }
    let staff: Vec<String> = body.staff_ids.iter().map(Uuid::to_string).collect();
    let msg_value = body.message.clone().map(Value::Object);
    let mut conn = db.conn().await?;
    let out = meetings::book_on_request(
        &mut conn,
        &user,
        title,
        py_datetime(&body.start),
        body.duration_minutes,
        &staff,
        body.customer_email.as_deref(),
        body.lead_id.map(|v| v.to_string()).as_deref(),
        body.deal_id.map(|v| v.to_string()).as_deref(),
        body.contact_id.map(|v| v.to_string()).as_deref(),
        msg_value.as_ref(),
    )
    .await?;
    Ok(resp::json(out))
}

/// `GET /meetings`.
pub async fn meetings_list(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows = meetings::upcoming(&mut conn).await?;
    Ok(resp::json(
        json!({"items": rows.iter().map(_meeting).collect::<Vec<_>>()}),
    ))
}

/// `_debrief_text(file, text)`: (text, source) from a typed note, an uploaded transcript, or a voice note.
async fn _debrief_text(
    file: Option<UploadFile>,
    text: Option<String>,
) -> ApiResult<(String, String)> {
    if let Some(t) = text.as_deref() {
        let t = pystr::strip(t);
        if !t.is_empty() {
            return Ok((t.to_string(), "typed debrief".to_string()));
        }
    }
    let Some(file) = file else {
        return Err(ApiError::http(
            400,
            "Send a voice note, a transcript file or the text",
        ));
    };
    let data = &file.data;
    if data.len() > MAX_UPLOAD {
        return Err(ApiError::http(413, "The file is too large (40 MB at most)"));
    }
    let name = pystr::lower(&file.filename);
    let ctype = file.content_type.clone().unwrap_or_default();
    if ctype.starts_with("audio/")
        || [".ogg", ".opus", ".mp3", ".m4a", ".wav", ".webm"]
            .iter()
            .any(|e| name.ends_with(e))
    {
        let filename = if file.filename.is_empty() {
            "debrief.ogg"
        } else {
            &file.filename
        };
        let mime = if ctype.is_empty() {
            "audio/ogg"
        } else {
            &ctype
        };
        let out = granary_services::stt_client::transcribe(
            data,
            granary_services::stt_client::TranscribeArgs {
                filename,
                mime,
                language: None,
                max_seconds: None,
            },
        )
        .await?;
        let text = match out.get("text") {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        return Ok((text, "Owner voice debrief".to_string()));
    }
    if [".txt", ".srt", ".vtt", ".docx", ".md"]
        .iter()
        .any(|e| name.ends_with(e))
    {
        return Ok((
            meetings::transcript_text(&name, data)?,
            format!("meeting transcript ({})", file.filename),
        ));
    }
    Err(ApiError::http(
        415,
        "Use a voice note (ogg, mp3, m4a, wav) or a transcript (txt, srt, vtt, docx)",
    ))
}

/// The `file: UploadFile | None = File(None), text: str | None = Form(None)` parameters.
fn debrief_form(form: &PyForm) -> ApiResult<(Option<UploadFile>, Option<String>)> {
    let mut cx = Cx::new();
    let file = form.opt_file(&mut cx, "file");
    let text = form.opt_text("text");
    form.check(cx)?;
    Ok((file, text))
}

/// `POST /meetings/{meeting_id}/debrief` (multipart).
pub async fn meeting_debrief(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(meeting_id): Path<String>,
    form: PyForm,
) -> ApiResult<Response> {
    let (file, text) = debrief_form(&form)?;
    _require(&user, "nary:approve")?;
    let id = py_uuid(&meeting_id)?;
    let mut conn = db.conn().await?;
    let Some(meeting) = load_meeting(&mut conn, id).await? else {
        return Err(ApiError::http(404, "Meeting not found"));
    };
    let (body, source) = _debrief_text(file, text).await?;
    let out = meetings::debrief(&mut conn, Some(&meeting), None, &body, &source, &user).await?;
    Ok(resp::json(out))
}

// ─── batch 3 R1: meeting recordings and notes files ────────────────────────────

/// A meeting-file upload: the `file` part, and for a file sent in pieces (Cloudflare cuts a request at
/// 100 MB) `upload_id`, `index` (from 0) and `total`.
struct FilePart {
    file: UploadFile,
    piece: Option<(Uuid, i64, i64)>,
}

fn file_form(form: &PyForm) -> ApiResult<FilePart> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    let upload_id = form.opt_text("upload_id");
    let index = form.opt_text("index");
    let total = form.opt_text("total");
    form.check(cx)?;
    let file = file.expect("checked");
    let piece = match upload_id.filter(|u| !u.trim().is_empty()) {
        None => None,
        Some(u) => {
            let bad = || {
                ApiError::http(
                    400,
                    "upload_id, index and total must be an id and two numbers",
                )
            };
            let id = Uuid::parse_str(u.trim()).map_err(|_| bad())?;
            let index: i64 = index
                .as_deref()
                .unwrap_or("")
                .trim()
                .parse()
                .map_err(|_| bad())?;
            let total: i64 = total
                .as_deref()
                .unwrap_or("")
                .trim()
                .parse()
                .map_err(|_| bad())?;
            Some((id, index, total))
        }
    };
    Ok(FilePart { file, piece })
}

/// Save a whole file, or one piece of it (the answer for a piece that is not the last: 202 with the count).
async fn save_file_part(
    conn: &mut sqlx::PgConnection,
    user: &granary_core::auth::AuthUser,
    meeting: Option<&NaryMeeting>,
    part: FilePart,
    source: &str,
) -> ApiResult<Result<meeting_files::MeetingFile, Value>> {
    let FilePart { file, piece } = part;
    match piece {
        None => Ok(Ok(meeting_files::save_upload(
            conn,
            user,
            meeting,
            &file.filename,
            file.content_type.as_deref(),
            &file.data,
            source,
        )
        .await?)),
        Some((upload_id, index, total)) => {
            match meeting_files::save_part(
                conn,
                user,
                meeting,
                upload_id,
                index,
                total,
                &file.filename,
                file.content_type.as_deref(),
                &file.data,
                source,
            )
            .await?
            {
                Some(row) => Ok(Ok(row)),
                None => Ok(Err(
                    json!({"status": "uploading", "received": index + 1, "total": total}),
                )),
            }
        }
    }
}

/// `POST /meetings/{meeting_id}/recording` (multipart `file`): a recording (or a notes file) of a meeting;
/// Nary transcribes and reads it in the background (GET /meeting-files shows the progress). The same
/// people who may upload a transcript (nary:approve).
pub async fn meeting_recording(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(meeting_id): Path<String>,
    form: PyForm,
) -> ApiResult<Response> {
    let part = file_form(&form)?;
    _require(&user, "nary:approve")?;
    let id = py_uuid(&meeting_id)?;
    let mut conn = db.conn().await?;
    let Some(meeting) = load_meeting(&mut conn, id).await? else {
        return Err(ApiError::http(404, "Meeting not found"));
    };
    Ok(
        match save_file_part(&mut conn, &user, Some(&meeting), part, "upload").await? {
            Ok(row) => resp::json_status(202, meeting_files::view(&row)),
            Err(progress) => resp::json_status(202, progress),
        },
    )
}

granary_core::py_model! {
    /// The query of `GET /meeting-files`.
    pub struct MeetingFilesQ {
        pub meeting_id: Option<Uuid> = None,
    }
}

/// `GET /meeting-files`: the newest meeting files (or one meeting's), with status and progress.
pub async fn meeting_files_list(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<MeetingFilesQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows = meeting_files::list(&mut conn, q.meeting_id, 200).await?;
    Ok(resp::json(
        json!({"items": rows.iter().map(meeting_files::view).collect::<Vec<_>>()}),
    ))
}

/// `POST /meeting-files` (multipart `file`): a notes file or recording attached in the web chat. Stored with
/// no meeting yet; the chat names the file_id so Nary can attach it (attach_meeting_notes).
pub async fn meeting_file_upload(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let part = file_form(&form)?;
    _require(&user, "nary:approve")?;
    let mut conn = db.conn().await?;
    let row = match save_file_part(&mut conn, &user, None, part, "web_chat").await? {
        Ok(row) => row,
        Err(progress) => return Ok(resp::json_status(202, progress)),
    };
    let mut out = meeting_files::view(&row);
    out["chat_line"] = json!(meeting_files::chat_line(&row));
    Ok(resp::json(out))
}

/// batch 4 D1: `POST /chat-files` (multipart `file`): any file attached in Nary's web chat. A meeting notes
/// file or recording from someone who may give Nary a transcript (nary:approve) goes to the meeting files, as
/// POST /meeting-files; any other file (a customer's PDF, a photo, a spreadsheet) is stored for the sender and
/// read with read_attachment. Answers the file with the line the chat sends with it (naming its file_id).
pub async fn chat_file_upload(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let part = file_form(&form)?;
    if !(user.is_superadmin || user.is_internal()) {
        return Err(ApiError::http(403, "Internal users only"));
    }
    let meeting_kind =
        meeting_files::kind_of(&part.file.filename, part.file.content_type.as_deref()).is_some();
    let mut conn = db.conn().await?;
    if meeting_kind && (user.is_superadmin || user.perms.contains("nary:approve")) {
        let row = match save_file_part(&mut conn, &user, None, part, "web_chat").await? {
            Ok(row) => row,
            Err(progress) => return Ok(resp::json_status(202, progress)),
        };
        let mut out = meeting_files::view(&row);
        out["chat_line"] = json!(meeting_files::chat_line(&row));
        return Ok(resp::json(out));
    }
    if part.piece.is_some_and(|(_, _, total)| total > 1) {
        let max = granary_services::nary::files::max_bytes(&mut conn).await?;
        return Err(ApiError::http(
            413,
            granary_services::nary::files::too_large_text(max),
        ));
    }
    let f = granary_services::nary::file_tools::register_chat_file(
        &mut conn,
        &user,
        &part.file.data,
        &part.file.filename,
        part.file.content_type.as_deref(),
        "",
    )
    .await?;
    let mut out = granary_services::nary::files::view(&f);
    out["id"] = json!(f.id.to_string());
    out["chat_line"] = json!(granary_services::nary::file_tools::chat_line(&f));
    Ok(resp::json(out))
}

/// `GET /meeting-files/{file_id}/transcript`: the transcript as a text file.
pub async fn meeting_file_transcript(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(file_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let id = py_uuid(&file_id)?;
    let mut conn = db.conn().await?;
    let (name, text) = meeting_files::transcript(&mut conn, id).await?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._- ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(resp::with_headers(
        resp::bytes_response(200, text, "text/plain; charset=utf-8"),
        &[(
            "content-disposition",
            format!("attachment; filename=\"{safe}\""),
        )],
    ))
}

granary_core::py_model! {
    /// `class DebriefText(BaseModel)`.
    pub struct DebriefText {
        pub text: String,
        pub meeting_id: Option<Uuid> = None,
        pub deal_id: Option<Uuid> = None,
        pub source: Option<String> = None,
    }
}

/// `POST /debrief/text`: a debrief or transcript as text (nary_debrief on WhatsApp or over MCP).
pub async fn debrief_text(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<DebriefText>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let mut conn = db.conn().await?;
    let meeting = match body.meeting_id {
        Some(id) => load_meeting(&mut conn, id).await?,
        None => None,
    };
    let deal = match body.deal_id {
        Some(id) => load_deal(&mut conn, id).await?,
        None => None,
    };
    if meeting.is_none() && deal.is_none() {
        return Err(ApiError::http(404, "No such meeting or deal"));
    }
    let text = pystr::strip(&body.text);
    if text.is_empty() {
        return Err(ApiError::http(400, "The debrief is empty"));
    }
    let source = head(
        body.source
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("Owner debrief (text)"),
        80,
    );
    let out = meetings::debrief(
        &mut conn,
        meeting.as_ref(),
        deal.as_ref(),
        text,
        &source,
        &user,
    )
    .await?;
    let mut m = match out {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert("label".into(), json!("Debrief read"));
    Ok(resp::json(Value::Object(m)))
}

/// `POST /deals/{deal_id}/debrief` (multipart): a debrief for a deal without a booked meeting.
pub async fn deal_debrief(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(deal_id): Path<String>,
    form: PyForm,
) -> ApiResult<Response> {
    let (file, text) = debrief_form(&form)?;
    _require(&user, "nary:approve")?;
    let id = py_uuid(&deal_id)?;
    let mut conn = db.conn().await?;
    let Some(deal) = load_deal(&mut conn, id).await? else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    let (body, source) = _debrief_text(file, text).await?;
    let out = meetings::debrief(&mut conn, None, Some(&deal), &body, &source, &user).await?;
    Ok(resp::json(out))
}

/// `POST /meetings/{meeting_id}/no-show`.
pub async fn meeting_no_show(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(meeting_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let id = py_uuid(&meeting_id)?;
    let mut conn = db.conn().await?;
    Ok(resp::json(meetings::no_show(&mut conn, id, &user).await?))
}

// ─── The Nary panel of a deal or lead ─────────────────────────────────────────

/// `_score(s)`.
pub fn _score(s: &NaryScore) -> Value {
    json!({
        "id": s.id.to_string(), "at": _iso(Some(&s.created_at)), "budget": s.budget, "authority": s.authority,
        "need": s.need, "timeline": s.timeline, "total": s.total, "class": s.class, "red_flags": s.red_flags,
        "green_flags": s.green_flags, "rationale": s.rationale, "intent": s.intent, "product": s.product,
        "model": s.model, "cascaded": s.cascaded,
    })
}

/// `_panel(db, deal=, lead=)`.
async fn _panel(
    db: &mut sqlx::PgConnection,
    deal: Option<Uuid>,
    lead: Option<Uuid>,
) -> ApiResult<Value> {
    // or_(*conds): deal_id first, then lead_id.
    let mut conds = vec![];
    let mut binds = vec![];
    if let Some(d) = deal {
        binds.push(d);
        conds.push(format!("deal_id = ${}", binds.len()));
    }
    if let Some(l) = lead {
        binds.push(l);
        conds.push(format!("lead_id = ${}", binds.len()));
    }
    let wh = conds.join(" OR ");
    let sql = format!("SELECT * FROM nary_scores WHERE {wh} ORDER BY created_at DESC LIMIT 20");
    let mut q = sqlx::query_as::<_, NaryScore>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let scores = q.fetch_all(&mut *db).await?;
    let sql = format!("SELECT * FROM nary_insights WHERE {wh} ORDER BY created_at DESC LIMIT 30");
    let mut q = sqlx::query_as::<_, NaryInsight>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let insights = q.fetch_all(&mut *db).await?;
    let sql = format!(
        "SELECT {MEETING_COLS} FROM nary_meetings WHERE {wh} ORDER BY created_at DESC LIMIT 10"
    );
    let mut q = sqlx::query_as::<_, NaryMeeting>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let meetings_rows = q.fetch_all(&mut *db).await?;
    let cadence_rows = match deal {
        Some(d) => cadence::timeline(db, Some(d), None).await?,
        None => vec![],
    };
    Ok(json!({
        "scores": scores.iter().map(_score).collect::<Vec<_>>(),
        "insights": insights.iter().map(|i| json!({"id": i.id.to_string(), "kind": i.kind, "status": i.status,
            "content": i.content, "at": _iso(Some(&i.created_at)), "model": i.model})).collect::<Vec<_>>(),
        "cadence": cadence_rows,
        "meetings": meetings_rows.iter().map(_meeting).collect::<Vec<_>>(),
    }))
}

/// `GET /deals/{deal_id}/panel`.
pub async fn deal_panel(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(deal_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let id = py_uuid(&deal_id)?;
    let mut conn = db.conn().await?;
    let Some(deal) = load_deal(&mut conn, id).await? else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    let lead: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM leads WHERE converted_to_deal_id = $1 LIMIT 1")
            .bind(deal.id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(resp::json(
        _panel(&mut conn, Some(deal.id), lead.map(|l| l.0)).await?,
    ))
}

/// `GET /leads/{lead_id}/panel`.
pub async fn lead_panel(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let id = py_uuid(&lead_id)?;
    let mut conn = db.conn().await?;
    let lead: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM leads WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some((lead,)) = lead else {
        return Err(ApiError::http(404, "Lead not found"));
    };
    Ok(resp::json(_panel(&mut conn, None, Some(lead)).await?))
}

/// `POST /deals/{deal_id}/meeting`: ask Nary to propose meeting slots for this deal now (a card).
pub async fn deal_meeting(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(deal_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let id = py_uuid(&deal_id)?;
    let mut conn = db.conn().await?;
    let Some(deal) = load_deal(&mut conn, id).await? else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    let chat: Option<WaChat> = match deal.contact_id {
        Some(cid) => {
            sqlx::query_as(
                "SELECT * FROM wa_chats WHERE contact_id = $1 AND status = 'active' LIMIT 1",
            )
            .bind(cid)
            .fetch_optional(&mut *conn)
            .await?
        }
        None => None,
    };
    let mut conv = match chat {
        Some(chat) => {
            let transcript = channels::conversation_text(&mut conn, &chat, None, 12).await?;
            replies::for_chat(&mut conn, &chat, &transcript).await?
        }
        None => {
            let mut conv = replies::Conversation::new("email", "");
            conv.contact = match deal.contact_id {
                Some(cid) => {
                    sqlx::query_as::<_, Contact>("SELECT * FROM contacts WHERE id = $1")
                        .bind(cid)
                        .fetch_optional(&mut *conn)
                        .await?
                }
                None => None,
            };
            replies::_locale(&mut conn, &mut conv).await?;
            conv
        }
    };
    conv.deal = Some(deal);
    let out = meetings::propose(&mut conn, &conv, None, "asked from the ERP", false).await?;
    match out {
        Some(v) => Ok(resp::json(v)),
        None => Err(ApiError::http(
            409,
            "No slot fits, or a meeting is already open for this deal",
        )),
    }
}

// ─── Report and SOP revisions ─────────────────────────────────────────────────

/// `GET /report`.
pub async fn report_view(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let data = report::collect(&mut conn, None).await?;
    let text = report::render_plain(&data);
    let mut m = match data {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert("text".into(), json!(text));
    Ok(resp::json(Value::Object(m)))
}

/// `POST /report/send`.
pub async fn report_send(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let data = report::send_weekly(&mut conn).await?;
    drop(conn);
    granary_services::approvals::notify::deliver_outbox_now().await;
    Ok(resp::json(
        json!({"sent": true, "week_ending": data.get("week_ending").cloned().unwrap_or(Value::Null)}),
    ))
}

/// `GET /sop-revisions`.
pub async fn sop_briefs(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let briefs = sop_revisions::briefs(&mut conn).await?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Uuid,
        String,
        Option<String>,
        Option<String>,
        Option<Uuid>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT id, title, status, version, previous_version_id, sop_code, updated_at FROM kb_articles \
         WHERE article_type = 'sop' AND is_deleted = false AND status IN ('draft', 'review') \
         AND version_notes IS NOT NULL ORDER BY updated_at DESC LIMIT 50",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(json!({
        "briefs": briefs.iter().map(|(k, v)| json!({"code": k, "brief": v})).collect::<Vec<_>>(),
        "drafts": rows.iter().map(|r| json!({"id": r.0.to_string(), "title": r.1, "status": r.2, "version": r.3,
            "previous_version_id": r.4.map(|v| v.to_string()), "sop_code": r.5,
            "updated_at": _iso(Some(&r.6))})).collect::<Vec<_>>(),
    })))
}

granary_core::py_model! {
    /// `class SopCodes(BaseModel)`.
    pub struct SopCodes {
        pub codes: Vec<String>,
    }
}

/// `POST /sop-revisions`: draft revisions in the background (`nary_sop_revisions.delay`).
pub async fn sop_draft(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SopCodes>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let known = sop_revisions::briefs(&mut conn).await?;
    drop(conn);
    let unknown: Vec<&str> = body
        .codes
        .iter()
        .filter(|c| !known.contains_key(c.as_str()))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() || body.codes.is_empty() {
        let list = unknown.join(", ");
        return Err(ApiError::http(
            400,
            format!(
                "No brief for: {}",
                if list.is_empty() {
                    "nothing selected"
                } else {
                    &list
                }
            ),
        ));
    }
    granary_services::tasks::delay(
        "nary_sop_revisions",
        vec![json!(body.codes), json!(user.id.to_string())],
        Map::new(),
    )
    .await?;
    Ok(resp::json(json!({ "queued": body.codes })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_matches_urllib() {
        assert_eq!(py_quote("a b/c", "/"), "a%20b/c");
        assert_eq!(py_quote("ü", "/"), "%C3%BC");
        assert_eq!(
            py_quote("http://x/y?a=b c&d=%20", ":/%#?=@[]!$&'()*+,;"),
            "http://x/y?a=b%20c&d=%20"
        );
    }

    #[test]
    fn uuid_like_python() {
        let u = Uuid::new_v4();
        assert_eq!(py_uuid(&u.to_string()).unwrap(), u);
        assert_eq!(py_uuid(&format!("{{{u}}}")).unwrap(), u);
        assert_eq!(py_uuid(&u.simple().to_string()).unwrap(), u);
        assert!(py_uuid("nope").is_err());
    }
}
