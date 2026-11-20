//! 29 Oct 2026 (Owner decisions): files your AI client or Nary put into the ERP.
//!
//! ```text
//!     upload_file      (Nary's chat; MCP at act and full power) content_base64 OR a public https url, file_name,
//!                      the record {entity_type: lead|deal|contact|organization, entity_id}
//!     preview_visual   save_to {entity_type, entity_id}: the rendered PNG is stored the same way
//! ```
//! The file becomes a normal CRM attachment of that record (its Attachments list), kept whether or not a message
//! ever carries it: uploader = the person, `uploaded_via` = 'mcp' (MCP) or 'nary' (Nary's chat), and one
//! audit_logs row. The returned file_id goes into propose_customer_message attach as {type: "file", id}.
//!
//! Rules: the type comes from the bytes (PNG, JPEG, WebP, GIF, PDF, DOCX, XLSX; never SVG, never a macro file),
//! not from the name or the claimed type; at most `nary.upload.max_mb`; the person must be allowed to upload to
//! the record on its page (attachments_access, the AttachmentPanel's rule). A URL is fetched through safe_fetch
//! (public addresses only, every redirect checked), https only, and read only up to the size cap.
//!
//! 1 Nov 2026 (Owner decisions): videos too. MP4 (an `ftyp` box with a video brand; H.264/AAC is what WhatsApp
//! plays), MOV (`qt  `) and WebM (EBML), never a HEIC/AVIF still image; at most `nary.upload.video_max_mb`
//! (images and documents keep `nary.upload.max_mb`), so a URL is read up to the larger cap and the type's own
//! cap is checked once the bytes are known. No transcoding: the length is read from the file itself (MP4
//! `moov/mvhd`, WebM `Segment/Info/Duration`), stored on the attachment (duration_seconds) and returned.

use std::time::Duration;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, StoreError};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::nary::outbound_files::human_size;

pub const MAX_KEY: &str = "nary.upload.max_mb";
/// 1 Nov 2026: the cap for videos.
pub const VIDEO_MAX_KEY: &str = "nary.upload.video_max_mb";
const WHERE: &str = "Nary & AI \u{2192} Nary and you on WhatsApp";

/// The record kinds a file may go on: (what the tool takes, the attachments entity_type).
pub const ENTITY_TYPES: [(&str, &str); 4] = [
    ("lead", "crm_lead"),
    ("deal", "crm_deal"),
    ("contact", "crm_contact"),
    ("organization", "crm_org"),
];

/// What the bytes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sniffed {
    pub mime: &'static str,
    pub ext: &'static str,
    /// For the card and the tool's answer ("PNG image", "PDF").
    pub label: &'static str,
}

pub const ALLOWED_TEXT: &str =
    "PNG, JPEG, WebP or GIF images, PDF, Word (.docx) or Excel (.xlsx) files, MP4, MOV or WebM videos";

/// ISO base media brands of an MP4 video (major or compatible brand).
const MP4_BRANDS: [&[u8; 4]; 11] = [
    b"isom", b"iso2", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1", b"M4V ", b"M4VP",
    b"dash",
];
/// Brands of still images in the same box format (HEIC, HEIF, AVIF): refused.
const IMAGE_BRANDS: [&[u8; 4]; 8] = [
    b"heic", b"heix", b"hevc", b"heim", b"mif1", b"msf1", b"avif", b"avis",
];

/// The `ftyp` brands (major first) of an ISO base media file, or None when it isn't one.
fn ftyp_brands(data: &[u8]) -> Option<Vec<[u8; 4]>> {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return None;
    }
    let size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
    let end = size.clamp(12, 4096).min(data.len());
    let mut brands: Vec<[u8; 4]> = vec![data[8..12].try_into().ok()?];
    // major brand, minor version (4), then the compatible brands
    let mut i = 16;
    while i + 4 <= end {
        brands.push(data[i..i + 4].try_into().ok()?);
        i += 4;
    }
    Some(brands)
}

/// The file's type from its first bytes (and, for a zip, its entries). `Err` says why it is refused.
pub fn sniff(data: &[u8]) -> Result<Sniffed, String> {
    let s = |mime, ext, label| Ok(Sniffed { mime, ext, label });
    if let Some(brands) = ftyp_brands(data) {
        let major = &brands[0];
        if IMAGE_BRANDS.contains(&major) {
            return Err(format!(
                "This file is a HEIC or AVIF picture: only {ALLOWED_TEXT} can be uploaded (save it as JPEG or PNG)."
            ));
        }
        if major == b"qt  " {
            return s("video/quicktime", "mov", "QuickTime video");
        }
        if brands.iter().any(|b| MP4_BRANDS.contains(&b)) {
            return s("video/mp4", "mp4", "MP4 video");
        }
        return Err(format!(
            "This file is an MP4-style file of a kind that is not allowed (brand {}): only {ALLOWED_TEXT} can \
             be uploaded.",
            String::from_utf8_lossy(major).trim()
        ));
    }
    if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        // EBML: WebM, not Matroska (.mkv), by its DocType
        let head = &data[..data.len().min(64)];
        if head.windows(4).any(|w| w == b"webm") {
            return s("video/webm", "webm", "WebM video");
        }
        return Err(format!(
            "This file is a Matroska (.mkv) video: only {ALLOWED_TEXT} can be uploaded."
        ));
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return s("image/png", "png", "PNG image");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return s("image/jpeg", "jpg", "JPEG image");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return s("image/gif", "gif", "GIF image");
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return s("image/webp", "webp", "WebP image");
    }
    if data.starts_with(b"%PDF-") {
        return s("application/pdf", "pdf", "PDF");
    }
    if data.starts_with(b"PK\x03\x04") {
        let names = crate::nary::file_text::zip_names(data)
            .map_err(|_| format!("This zip file can't be uploaded: only {ALLOWED_TEXT}."))?;
        let has = |n: &str| names.iter().any(|x| x == n);
        if !has("[Content_Types].xml") {
            return Err(format!(
                "This zip file can't be uploaded: only {ALLOWED_TEXT}."
            ));
        }
        if names.iter().any(|n| n.ends_with("vbaProject.bin")) {
            return Err("Office files with macros can't be uploaded.".into());
        }
        if has("word/document.xml") {
            return s(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "docx",
                "Word document",
            );
        }
        if has("xl/workbook.xml") {
            return s(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "xlsx",
                "Excel workbook",
            );
        }
        return Err(format!(
            "This Office file can't be uploaded: only {ALLOWED_TEXT}."
        ));
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(512)]).to_ascii_lowercase();
    let what = if data.starts_with(b"MZ") || data.starts_with(b"\x7fELF") {
        "a program"
    } else if head.contains("<svg") {
        "an SVG drawing"
    } else if head.contains("<html") || head.contains("<!doctype") || head.contains("<script") {
        "a web page"
    } else {
        "of a type that is not allowed"
    };
    Err(format!(
        "This file is {what}, judged by its content: only {ALLOWED_TEXT} can be uploaded."
    ))
}

/// 1 Nov 2026: a video (MP4, MOV, WebM).
pub fn is_video(mime: &str) -> bool {
    mime.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .starts_with("video/")
}

/// "0:42", "1:00", "1:02:03".
pub fn duration_label(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The length of a video in seconds, read from the file without decoding it: an MP4/MOV's `moov/mvhd`
/// (timescale and duration), a WebM's `Segment/Info` (TimecodeScale and Duration). None when it isn't there.
pub fn video_duration(data: &[u8]) -> Option<f64> {
    let d = if ftyp_brands(data).is_some() {
        mp4_duration(data)
    } else if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        webm_duration(data)
    } else {
        None
    }?;
    (d.is_finite() && d >= 0.0).then_some(d)
}

/// The ISO boxes in `data`: (type, body).
fn mp4_boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let size32 = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as u64;
        let kind: [u8; 4] = [data[i + 4], data[i + 5], data[i + 6], data[i + 7]];
        let (header, size) = match size32 {
            0 => (8u64, (data.len() - i) as u64),
            1 => {
                let Some(big) = data.get(i + 8..i + 16) else {
                    break;
                };
                (16, u64::from_be_bytes(big.try_into().unwrap_or([0; 8])))
            }
            n => (8, n),
        };
        if size < header {
            break;
        }
        let start = i + header as usize;
        let end = (i as u64).saturating_add(size).min(data.len() as u64) as usize;
        if start > end {
            break;
        }
        out.push((kind, &data[start..end]));
        i = end;
    }
    out
}

/// 1 Nov 2026: false for an MP4 whose video track is HEVC, AV1 or VP9 (no H.264 `avc1` sample entry):
/// WhatsApp Web would not play it, so it goes as a document. Anything not parsed counts as playable.
pub fn mp4_plays_on_whatsapp(data: &[u8]) -> bool {
    let Some(moov) = mp4_boxes(data)
        .into_iter()
        .find(|(k, _)| k == b"moov")
        .map(|b| b.1)
    else {
        return true;
    };
    let has = |tag: &[u8; 4]| moov.windows(4).any(|w| w == tag);
    has(b"avc1") || !(has(b"hvc1") || has(b"hev1") || has(b"av01") || has(b"vp09"))
}

fn mp4_duration(data: &[u8]) -> Option<f64> {
    let moov = mp4_boxes(data).into_iter().find(|(k, _)| k == b"moov")?.1;
    let mvhd = mp4_boxes(moov).into_iter().find(|(k, _)| k == b"mvhd")?.1;
    let be32 = |at: usize| -> Option<u64> {
        Some(u32::from_be_bytes(mvhd.get(at..at + 4)?.try_into().ok()?) as u64)
    };
    let (timescale, duration) = match mvhd.first()? {
        // version, flags (3), creation (8), modification (8), timescale (4), duration (8)
        1 => (
            be32(20)?,
            u64::from_be_bytes(mvhd.get(24..32)?.try_into().ok()?),
        ),
        // version, flags (3), creation (4), modification (4), timescale (4), duration (4)
        _ => (be32(12)?, be32(16)?),
    };
    // all ones = unknown
    if timescale == 0 || duration == u64::MAX || duration == u64::from(u32::MAX) {
        return None;
    }
    Some(duration as f64 / timescale as f64)
}

/// An EBML variable-length number at `data[i..]`: (value, length). `keep_marker` for element ids.
fn ebml_vint(data: &[u8], i: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let first = *data.get(i)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || i + len > data.len() {
        return None;
    }
    let mut v: u64 = if keep_marker {
        u64::from(first)
    } else {
        u64::from(first) & ((1u64 << (8 - len)) - 1)
    };
    for b in &data[i + 1..i + len] {
        v = (v << 8) | u64::from(*b);
    }
    Some((v, len))
}

/// The children of an EBML element body: (id, body); an unknown size runs to the end.
fn ebml_children(data: &[u8]) -> Vec<(u64, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let Some((id, il)) = ebml_vint(data, i, true) else {
            break;
        };
        let Some((size, sl)) = ebml_vint(data, i + il, false) else {
            break;
        };
        let start = i + il + sl;
        let unknown = size == (1u64 << (7 * sl)) - 1;
        let end = if unknown {
            data.len()
        } else {
            start
                .saturating_add(usize::try_from(size).unwrap_or(usize::MAX))
                .min(data.len())
        };
        if start > end {
            break;
        }
        out.push((id, &data[start..end]));
        i = end;
    }
    out
}

fn webm_duration(data: &[u8]) -> Option<f64> {
    const SEGMENT: u64 = 0x1853_8067;
    const INFO: u64 = 0x1549_A966;
    const TIMECODE_SCALE: u64 = 0x2A_D7B1;
    const DURATION: u64 = 0x4489;
    let segment = ebml_children(data)
        .into_iter()
        .find(|(id, _)| *id == SEGMENT)?
        .1;
    let info = ebml_children(segment)
        .into_iter()
        .find(|(id, _)| *id == INFO)?
        .1;
    let mut scale: u64 = 1_000_000;
    let mut duration: Option<f64> = None;
    for (id, body) in ebml_children(info) {
        if id == TIMECODE_SCALE && !body.is_empty() && body.len() <= 8 {
            scale = body.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
        } else if id == DURATION {
            duration = match body.len() {
                4 => Some(f64::from(f32::from_be_bytes(body.try_into().ok()?))),
                8 => Some(f64::from_be_bytes(body.try_into().ok()?)),
                _ => None,
            };
        }
    }
    Some(duration? * scale as f64 / 1e9)
}

/// A picture WhatsApp shows inline (sent as a photo, not as a document).
pub fn is_picture(mime: &str) -> bool {
    matches!(
        mime.split(';').next().unwrap_or("").trim(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    )
}

/// The short type name a card shows ("PNG image", "PDF", "Word document").
pub fn type_label(mime: &str) -> String {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        "image/png" => "PNG image",
        "image/jpeg" => "JPEG image",
        "image/webp" => "WebP image",
        "image/gif" => "GIF image",
        "application/pdf" => "PDF",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            "Word document"
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "Excel workbook",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "PowerPoint",
        "text/plain" => "text",
        "video/mp4" => "MP4 video",
        "video/quicktime" => "QuickTime video",
        "video/webm" => "WebM video",
        "" => "file",
        other => return other.to_string(),
    }
    .to_string()
}

/// The largest upload, in bytes (nary.upload.max_mb). `Ok(Err)` names the key when it is missing or wrong.
pub async fn max_bytes(db: &mut PgConnection) -> ApiResult<Result<i64, String>> {
    match require_int(db, MAX_KEY, WHERE).await {
        Ok(mb) => Ok(Ok(mb.max(0) * 1024 * 1024)),
        Err(StoreError::Setting(e)) => Ok(Err(e.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// 1 Nov 2026: the video cap in bytes (nary.upload.video_max_mb), as [`max_bytes`].
pub async fn video_max_bytes(db: &mut PgConnection) -> ApiResult<Result<i64, String>> {
    match require_int(db, VIDEO_MAX_KEY, WHERE).await {
        Ok(mb) => Ok(Ok(mb.max(0) * 1024 * 1024)),
        Err(StoreError::Setting(e)) => Ok(Err(e.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// How much to read before the type is known (base64 or a download): the larger of the two caps. The type's
/// own cap is checked by [`store`].
pub async fn read_cap(db: &mut PgConnection) -> ApiResult<Result<i64, String>> {
    let files = match max_bytes(db).await? {
        Ok(m) => m,
        Err(why) => return Ok(Err(why)),
    };
    let videos = match video_max_bytes(db).await? {
        Ok(m) => m,
        Err(why) => return Ok(Err(why)),
    };
    Ok(Ok(files.max(videos)))
}

/// A record a file may go on: the attachments entity_type and the id, once the person is allowed to upload
/// there (the AttachmentPanel's rule). `Ok(Err)` is told to the model.
pub async fn target(
    db: &mut PgConnection,
    user: &AuthUser,
    entity_type: &str,
    entity_id: &str,
) -> ApiResult<Result<(&'static str, &'static str, Uuid), String>> {
    let wanted = entity_type.trim().to_ascii_lowercase();
    let Some((public, etype)) = ENTITY_TYPES
        .iter()
        .find(|(p, e)| *p == wanted || *e == wanted)
        .copied()
    else {
        return Ok(Err(
            "entity_type is one of lead, deal, contact, organization".into(),
        ));
    };
    let Ok(id) = Uuid::parse_str(entity_id.trim()) else {
        return Ok(Err(format!("entity_id '{entity_id}' is not a {public} id")));
    };
    match crate::attachments_access::assert_can_access(db, user, etype, &id.to_string()).await {
        Ok(_) => Ok(Ok((public, etype, id))),
        Err(ApiError::Http { status, detail, .. }) => {
            let why = match detail {
                Value::String(s) => s,
                other => other.to_string(),
            };
            Ok(Err(if status.as_u16() == 404 {
                format!("No {public} with id {id}")
            } else {
                format!("You can't add files to this {public}: {why}")
            }))
        }
        Err(e) => Err(e),
    }
}

/// A file name to keep: the last path part, no control characters, at most 200 characters, ending in the
/// extension of what the bytes are.
pub fn clean_name(raw: &str, sniffed: &Sniffed) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut name: String = base
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    let lower = name.to_ascii_lowercase();
    let ok_ext: Vec<&str> = if sniffed.ext == "jpg" {
        vec!["jpg", "jpeg"]
    } else {
        vec![sniffed.ext]
    };
    let has_ext = lower
        .rsplit_once('.')
        .is_some_and(|(_, e)| ok_ext.contains(&e));
    if !has_ext {
        // a wrong extension goes (render.svg holding a PNG becomes render.png); a name without one gets it
        if let Some((stem, ext)) = name.rsplit_once('.') {
            if !stem.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric())
            {
                name = stem.to_string();
            }
        }
        if name.is_empty() {
            name = "file".into();
        }
        name = format!("{name}.{}", sniffed.ext);
    }
    if name.chars().count() > 200 {
        let ext = format!(".{}", name.rsplit('.').next().unwrap_or(sniffed.ext));
        let stem: String = name.chars().take(200 - ext.chars().count()).collect();
        name = format!("{stem}{ext}");
    }
    name
}

/// 'mcp' (MCP) or 'nary' (Nary's chat): the marker the Attachments list shows.
pub fn via_of(args: &Value) -> &'static str {
    match args.get(VIA_ARG).and_then(Value::as_str) {
        Some("mcp") => "mcp",
        _ => "nary",
    }
}

/// 5 Nov 2026: `uploaded_via` of a file dropped on an upload link (crate::upload_links).
pub const UPLOAD_LINK_VIA: &str = "upload_link";

/// "an MCP client", "Nary" or "an upload link": who put the file there, for the audit note.
fn via_label(via: &str) -> &'static str {
    match via {
        "mcp" => "an MCP client",
        UPLOAD_LINK_VIA => "an upload link",
        _ => "Nary",
    }
}

/// Set by the MCP router on upload_file and preview_visual: the file came from an MCP client.
pub const VIA_ARG: &str = "_via";

/// Stores the bytes on the record (checked with [`target`] by the caller or here) and returns the tool's answer:
/// {file_id, file_name, size, size_label, mime_type, type, entity, note}. `Ok(Err)` is told to the model.
pub async fn store(
    db: &mut PgConnection,
    user: &AuthUser,
    entity_type: &str,
    entity_id: &str,
    file_name: &str,
    data: Vec<u8>,
    via: &str,
) -> ApiResult<Result<Value, String>> {
    let (public, etype, id) = match target(db, user, entity_type, entity_id).await? {
        Ok(t) => t,
        Err(why) => return Ok(Err(why)),
    };
    if data.is_empty() {
        return Ok(Err("The file is empty".into()));
    }
    let sniffed = match sniff(&data) {
        Ok(s) => s,
        Err(why) => return Ok(Err(why)),
    };
    // 1 Nov 2026: a video has its own cap
    let video = is_video(sniffed.mime);
    let (max, key) = if video {
        (video_max_bytes(db).await?, VIDEO_MAX_KEY)
    } else {
        (max_bytes(db).await?, MAX_KEY)
    };
    let max = match max {
        Ok(m) => m,
        Err(why) => return Ok(Err(why)),
    };
    if data.len() as i64 > max {
        return Ok(Err(format!(
            "The {} is {}, over the {} one upload may be ({key}).",
            if video { "video" } else { "file" },
            human_size(data.len() as i64),
            human_size(max)
        )));
    }
    let duration = if video { video_duration(&data) } else { None };
    let name = clean_name(file_name, &sniffed);
    let file_id = Uuid::new_v4();
    let object_key = format!("attachments/{etype}/{id}/{}/{name}", Uuid::new_v4());
    let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
    let store = crate::storage::minio();
    crate::storage::ensure_bucket(store.as_ref(), &bucket)
        .await
        .map_err(ApiError::internal)?;
    store
        .put_object(&bucket, &object_key, &data, sniffed.mime)
        .await
        .map_err(ApiError::internal)?;
    let file_type = if sniffed.mime.starts_with("image/") {
        "image"
    } else if video {
        "video"
    } else {
        "file"
    };
    let size = data.len() as i64;
    let detail = json!({"entity_type": etype, "entity_id": id.to_string(), "file_name": name,
                        "size": size, "mime_type": sniffed.mime, "via": via,
                        "duration_seconds": duration});
    let row = Row {
        file_id,
        etype,
        entity_id: id,
        user,
        name: &name,
        size,
        mime: sniffed.mime,
        object_key: &object_key,
        file_type,
        via,
        duration,
        detail: &detail,
        note: format!("Uploaded via {} to {public} {id}", via_label(via)),
    };
    // its own transaction: the file is kept even when the chat turn or the message fails later
    match crate::nary::session_pool() {
        Ok(pool) => {
            let mut tx = pool.begin().await?;
            insert_row(&mut tx, &row).await?;
            tx.commit().await?;
        }
        Err(_) => insert_row(db, &row).await?,
    }
    tracing::info!(file_id = %file_id, entity_type = etype, entity_id = %id, user_id = %user.id, via,
                   size, mime = sniffed.mime, "nary_file_uploaded");
    let how = if is_picture(sniffed.mime) {
        "; on WhatsApp a picture goes as a photo"
    } else if video {
        "; on WhatsApp an MP4 up to nary.wa.video_max_mb goes as a playable video and a larger video as a \
         document; an email carries it up to nary.attach.max_total_mb, else a download link"
    } else {
        ""
    };
    let mut out = json!({
        "file_id": file_id.to_string(),
        "file_name": name,
        "size": size,
        "size_label": human_size(size),
        "mime_type": sniffed.mime,
        "type": sniffed.label,
        "entity": {"type": public, "id": id.to_string()},
        "note": format!(
            "Stored on the {public}'s Attachments (kept whether or not a message carries it). To send it, pass \
             attach [{{\"type\": \"file\", \"id\": \"{file_id}\"}}] to propose_customer_message{how}."
        ),
    });
    if video {
        out["duration_seconds"] = json!(duration);
        out["duration_label"] = json!(duration.map(duration_label));
    }
    Ok(Ok(out))
}

struct Row<'a> {
    file_id: Uuid,
    etype: &'a str,
    entity_id: Uuid,
    user: &'a AuthUser,
    name: &'a str,
    size: i64,
    mime: &'a str,
    object_key: &'a str,
    file_type: &'a str,
    via: &'a str,
    duration: Option<f64>,
    detail: &'a Value,
    note: String,
}

/// The attachment row and its audit_logs row.
async fn insert_row(conn: &mut PgConnection, r: &Row<'_>) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, uploaded_by_id, file_name, file_size, mime_type, \
         object_key, url, file_type, uploaded_via, duration_seconds) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '', $9, $10, $11)",
    )
    .bind(r.file_id)
    .bind(r.etype)
    .bind(r.entity_id.to_string())
    .bind(r.user.id)
    .bind(r.name)
    .bind(r.size)
    .bind(r.mime)
    .bind(r.object_key)
    .bind(r.file_type)
    .bind(r.via)
    .bind(r.duration)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         new_values, notes, origin) VALUES ($1, $2, $3, $9, 'attachment', $4, $5, $6::json, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(r.user.id)
    .bind(&r.user.name)
    .bind(r.file_id.to_string())
    .bind(r.name.chars().take(200).collect::<String>())
    .bind(r.detail.to_string())
    .bind(&r.note)
    .bind(match r.via {
        "mcp" => "mcp",
        UPLOAD_LINK_VIA => UPLOAD_LINK_VIA,
        _ => "nary",
    })
    // 5 Nov 2026: a file dropped on an upload link has its own action
    .bind(if r.via == UPLOAD_LINK_VIA {
        "attachment.upload_via_link"
    } else {
        "attachment.upload_via_ai"
    })
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The bytes of `content_base64` (plain or a data: URL, line breaks allowed), refused early when the text alone
/// is clearly over `max`.
pub fn decode_base64(raw: &str, max: i64) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let body = match raw.trim().strip_prefix("data:") {
        Some(rest) => rest.split_once(',').map(|(_, b)| b).unwrap_or(""),
        None => raw.trim(),
    };
    let clean: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    if (clean.len() as i64 / 4) * 3 > max + 3 {
        return Err(over_cap(max));
    }
    base64::engine::general_purpose::STANDARD
        .decode(clean.as_bytes())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(clean.as_bytes()))
        .or_else(|_| {
            base64::engine::general_purpose::STANDARD_NO_PAD.decode(clean.trim_end_matches('='))
        })
        .map_err(|_| "content_base64 is not valid base64".to_string())
}

/// The refusal of a file over the read cap (before its type is known).
fn over_cap(max: i64) -> String {
    format!(
        "The file is over the {} one upload may be ({MAX_KEY}; {VIDEO_MAX_KEY} for videos).",
        human_size(max)
    )
}

/// Downloads a public https file through safe_fetch, reading at most `max` bytes. Returns the bytes and the
/// file name the address ends with.
pub async fn fetch(url: &str, max: i64) -> Result<(Vec<u8>, String), String> {
    let parsed =
        reqwest::Url::parse(url.trim()).map_err(|_| format!("'{url}' is not a web address"))?;
    let loopback_test = crate::safe_fetch::loopback_allowed()
        && parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1") | Some("localhost"));
    if parsed.scheme() != "https" && !loopback_test {
        return Err("Only https addresses are downloaded.".into());
    }
    let mut resp = crate::safe_fetch::get(parsed.as_str(), &[], &[], Duration::from_secs(60), 5)
        .await
        .map_err(|e| format!("The file could not be downloaded: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!(
            "The file could not be downloaded: the address answered {status}"
        ));
    }
    let over = || over_cap(max);
    if resp.content_length().is_some_and(|n| n as i64 > max) {
        return Err(over());
    }
    let final_url = resp.url().clone();
    let mut data: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("The file could not be downloaded: {e}"))?
    {
        if (data.len() + chunk.len()) as i64 > max {
            return Err(over());
        }
        data.extend_from_slice(&chunk);
    }
    let name = final_url
        .path_segments()
        .and_then(|mut s| s.next_back().map(str::to_string))
        .filter(|s| !s.is_empty())
        .map(|s| {
            url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or(s)
        })
        .unwrap_or_else(|| "file".into());
    Ok((data, name))
}
