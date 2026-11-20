//! 5 Nov 2026 (Owner-approved plan, Stream B): one-time upload links, so files from an AI chat reach the ERP.
//!
//! ```text
//!     create_upload_link   (Nary's chat; MCP at act and full power) a link for a lead, deal, contact or
//!                          organization, or 'staging' (no record): a random 32-byte token, shown once, kept as
//!                          its sha256; works files.upload_link.minutes, takes files.upload_link.max_files files
//!     /drop/<token>        the drop page (no login; works on a phone): drop files, watch them go up, Done
//!     PUT /api/v1/public/drop/<token>/files?name=   the raw bytes (curl -T), read up to uploads::read_cap
//!     get_upload_link      the files a link took, with file_ids for propose_customer_message attach
//!     revoke_upload_link   closes it (the page's Done does the same)
//! ```
//! A file on a record target is stored exactly as upload_file stores one (nary::uploads::store: the type from
//! the bytes, the type's cap, the record's attachment rule), AS THE LINK'S CREATOR: uploader = the creator,
//! uploaded_via 'upload_link', audit 'attachment.upload_via_link'. A staging file becomes a Nary file of the
//! creator's (nary_files, source 'chat', chat_user_id = the creator): only the creator (or a superadmin) may
//! send it on, as with a file sent in the chat. Every file is recorded in upload_link_files and counted on the
//! link under a row lock (SELECT ... FOR UPDATE), so two uploads at once never pass max_files.
//!
//! The public routes never show other ERP data: the record's kind and name, the link's limits and the names
//! and sizes of the files it took.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::crypto::tokens::{token_urlsafe, OsRandom};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, StoreError};

use crate::nary::files::{self, Origin};
use crate::nary::outbound_files::human_size;
use crate::nary::uploads::{self, UPLOAD_LINK_VIA};

pub const MINUTES_KEY: &str = "files.upload_link.minutes";
pub const MAX_FILES_KEY: &str = "files.upload_link.max_files";
/// Requests per minute, per address and per link, on the public routes.
pub const RATE_KEY: &str = "files.upload_link.requests_per_minute";
const WHERE: &str = "Nary & AI \u{2192} Nary and you on WhatsApp";
/// The public routes' prefix (the router mounts them here; the tool's upload_url points here).
pub const PUBLIC_PREFIX: &str = "/api/v1/public/drop";
/// The drop page of the web app.
pub const PAGE_PREFIX: &str = "/drop";

/// What a link may target: the record kinds of upload_file plus 'staging'.
pub const TARGETS: [&str; 5] = ["lead", "deal", "contact", "organization", "staging"];

/// One row of upload_links.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Link {
    pub id: Uuid,
    pub target: String,
    pub entity_id: Option<Uuid>,
    pub note: Option<String>,
    pub created_by_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub max_files: i32,
    pub files_used: i32,
    pub revoked_at: Option<DateTime<Utc>>,
}

const COLS: &str =
    "id, target, entity_id, note, created_by_id, created_at, expires_at, max_files, files_used, \
                    revoked_at";

/// One row of upload_link_files.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkFile {
    pub file_id: Uuid,
    pub file_kind: String,
    pub name: String,
    pub size: i64,
    pub mime: String,
    pub duration_seconds: Option<f64>,
    pub created_at: DateTime<Utc>,
}

/// The stored form of a token.
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.trim().as_bytes()))
}

fn iso(t: &DateTime<Utc>) -> String {
    granary_compat::pyjson::PyDateTime::from_chrono_utc(t).isoformat()
}

/// An int setting; `Ok(Err)` names the key when it is missing or wrong.
async fn setting_int(db: &mut PgConnection, key: &str) -> ApiResult<Result<i64, String>> {
    match require_int(db, key, WHERE).await {
        Ok(v) => Ok(Ok(v)),
        Err(StoreError::Setting(e)) => Ok(Err(e.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// The public address of the web app (APP_URL), the base of the links.
pub fn base_url() -> String {
    granary_core::config::settings()
        .APP_URL
        .trim_end_matches('/')
        .to_string()
}

/// The two caps for the page and the tool: {file_max_mb, video_max_mb, allowed}.
async fn caps(db: &mut PgConnection) -> ApiResult<Result<Value, String>> {
    let files = match uploads::max_bytes(db).await? {
        Ok(m) => m,
        Err(why) => return Ok(Err(why)),
    };
    let videos = match uploads::video_max_bytes(db).await? {
        Ok(m) => m,
        Err(why) => return Ok(Err(why)),
    };
    Ok(Ok(json!({
        "allowed": uploads::ALLOWED_TEXT,
        "file_max_bytes": files,
        "file_max_label": human_size(files),
        "video_max_bytes": videos,
        "video_max_label": human_size(videos),
    })))
}

/// The record's kind and name ("Deal: PT X"); "Staging (no record yet)" for staging.
async fn target_label(db: &mut PgConnection, link: &Link) -> ApiResult<String> {
    let Some(id) = link.entity_id else {
        return Ok("Staging (files for a message, not on a record yet)".into());
    };
    let sql = match link.target.as_str() {
        "lead" => "SELECT title FROM leads WHERE id = $1",
        "deal" => "SELECT title FROM deals WHERE id = $1",
        "organization" => "SELECT name FROM organizations WHERE id = $1",
        _ => {
            "SELECT trim(coalesce(first_name, '') || ' ' || coalesce(last_name, '')) FROM contacts WHERE id = $1"
        }
    };
    let name: Option<String> = sqlx::query_scalar(sql)
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    let kind = match link.target.as_str() {
        "lead" => "Lead",
        "deal" => "Deal",
        "organization" => "Organization",
        _ => "Contact",
    };
    Ok(format!(
        "{kind}: {}",
        name.filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "(no name)".into())
    ))
}

/// Why a link takes no more files, if it doesn't.
pub fn closed_reason(link: &Link, now: DateTime<Utc>) -> Option<String> {
    if link.revoked_at.is_some() {
        return Some(
            "This upload link was closed. Ask for a new one if you have more files.".into(),
        );
    }
    if link.expires_at <= now {
        return Some(format!(
            "This upload link expired (links work for {MINUTES_KEY} minutes). Ask for a new one."
        ));
    }
    if link.files_used >= link.max_files {
        return Some(format!(
            "This upload link has taken its {} files. Ask for a new one.",
            link.max_files
        ));
    }
    None
}

fn state(link: &Link, now: DateTime<Utc>) -> &'static str {
    if link.revoked_at.is_some() {
        "closed"
    } else if link.expires_at <= now {
        "expired"
    } else if link.files_used >= link.max_files {
        "full"
    } else {
        "open"
    }
}

/// Makes a link. `entity_id` is required for a record and refused for staging; the person must be allowed to
/// add files to the record (upload_file's rule). Returns the tool's answer with the token in its URLs (shown
/// only here). `Ok(Err)` is told to the model.
pub async fn create(
    db: &mut PgConnection,
    user: &AuthUser,
    target: &str,
    entity_id: &str,
    note: &str,
    origin: &str,
) -> ApiResult<Result<Value, String>> {
    let wanted = target.trim().to_ascii_lowercase();
    if !TARGETS.contains(&wanted.as_str()) {
        return Ok(Err(
            "entity_type is one of lead, deal, contact, organization or staging".into(),
        ));
    }
    let entity: Option<Uuid> = if wanted == "staging" {
        if !entity_id.trim().is_empty() {
            return Ok(Err(
                "staging takes no entity_id (the files wait for a message, not on a record)".into(),
            ));
        }
        None
    } else {
        if entity_id.trim().is_empty() {
            return Ok(Err(format!("entity_id (the {wanted}'s id) is required")));
        }
        match uploads::target(db, user, &wanted, entity_id).await? {
            Ok((_, _, id)) => Some(id),
            Err(why) => return Ok(Err(why)),
        }
    };
    let minutes = match setting_int(db, MINUTES_KEY).await? {
        Ok(v) => v.max(1),
        Err(why) => return Ok(Err(why)),
    };
    let max_files = match setting_int(db, MAX_FILES_KEY).await? {
        Ok(v) => v.clamp(1, 1000) as i32,
        Err(why) => return Ok(Err(why)),
    };
    let caps = match caps(db).await? {
        Ok(c) => c,
        Err(why) => return Ok(Err(why)),
    };
    let token = token_urlsafe(&mut OsRandom, 32);
    let id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::minutes(minutes);
    let note: String = note.trim().chars().take(500).collect();
    sqlx::query(
        "INSERT INTO upload_links (id, token_hash, target, entity_id, note, created_by_id, expires_at, max_files) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(token_hash(&token))
    .bind(&wanted)
    .bind(entity)
    .bind((!note.is_empty()).then_some(&note))
    .bind(user.id)
    .bind(expires_at)
    .bind(max_files)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         new_values, notes, origin) VALUES ($1, $2, $3, 'upload_link.create', 'upload_link', $4, $5, $6::json, \
         $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(&user.name)
    .bind(id.to_string())
    .bind(&wanted)
    .bind(
        json!({"target": wanted, "entity_id": entity.map(|e| e.to_string()), "max_files": max_files,
               "expires_at": iso(&expires_at)})
        .to_string(),
    )
    .bind(&note)
    .bind(origin)
    .execute(&mut *db)
    .await?;
    let base = base_url();
    let page_url = format!("{base}{PAGE_PREFIX}/{token}");
    let put_base = format!("{base}{PUBLIC_PREFIX}/{token}/files");
    Ok(Ok(json!({
        "link_id": id.to_string(),
        "target": wanted,
        "entity_id": entity.map(|e| e.to_string()),
        "page_url": page_url,
        "upload_url": format!("{put_base}?name=<file name>"),
        "expires_at": iso(&expires_at),
        "minutes": minutes,
        "max_files": max_files,
        "allowed_types": caps["allowed"],
        "caps": {"file_max": caps["file_max_label"], "video_max": caps["video_max_label"]},
        "how_to": {
            "owner": format!(
                "Open {page_url} on your phone or computer, drop the files there and press Done (the link \
                 works {minutes} minutes and takes {max_files} files)."
            ),
            "curl": format!("curl -T \"video.mp4\" \"{put_base}?name=video.mp4\""),
        },
        "next": "Give the Owner page_url. When they say the files are up, call get_upload_link with link_id: \
                 its files have the file_ids for propose_customer_message attach {\"type\": \"file\", \"id\"}.",
    })))
}

/// A link by id, for its creator (or a superadmin). `Ok(Err)` is told to the model.
async fn own_link(
    db: &mut PgConnection,
    user: &AuthUser,
    link_id: &str,
) -> ApiResult<Result<Link, String>> {
    let Ok(id) = Uuid::parse_str(link_id.trim()) else {
        return Ok(Err(format!("'{link_id}' is not an upload link id")));
    };
    let link: Option<Link> =
        sqlx::query_as(&format!("SELECT {COLS} FROM upload_links WHERE id = $1"))
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    match link {
        Some(l) if l.created_by_id == user.id || user.is_superadmin => Ok(Ok(l)),
        _ => Ok(Err(format!("No upload link with id {id}"))),
    }
}

/// The files a link took, oldest first.
pub async fn link_files(db: &mut PgConnection, link_id: Uuid) -> ApiResult<Vec<LinkFile>> {
    Ok(sqlx::query_as(
        "SELECT file_id, file_kind, name, size, mime, duration_seconds, created_at FROM upload_link_files \
         WHERE link_id = $1 ORDER BY created_at, id",
    )
    .bind(link_id)
    .fetch_all(&mut *db)
    .await?)
}

/// `get_upload_link`: the link's state and its files with their file_ids.
pub async fn get(
    db: &mut PgConnection,
    user: &AuthUser,
    link_id: &str,
) -> ApiResult<Result<Value, String>> {
    let link = match own_link(db, user, link_id).await? {
        Ok(l) => l,
        Err(why) => return Ok(Err(why)),
    };
    let files: Vec<Value> = link_files(db, link.id)
        .await?
        .iter()
        .map(|f| {
            let mut v = json!({
                "file_id": f.file_id.to_string(),
                "stored_as": if f.file_kind == "attachment" { "record attachment" } else { "staging (Nary file)" },
                "name": f.name,
                "size": f.size,
                "size_label": human_size(f.size),
                "mime_type": f.mime,
                "type": uploads::type_label(&f.mime),
                "uploaded_at": iso(&f.created_at),
            });
            if f.duration_seconds.is_some() || uploads::is_video(&f.mime) {
                v["duration_seconds"] = json!(f.duration_seconds);
                v["duration_label"] = json!(f.duration_seconds.map(uploads::duration_label));
            }
            v
        })
        .collect();
    let now = Utc::now();
    Ok(Ok(json!({
        "link_id": link.id.to_string(),
        "target": link.target,
        "entity_id": link.entity_id.map(|e| e.to_string()),
        "label": target_label(db, &link).await?,
        "note": link.note,
        "state": state(&link, now),
        "expires_at": iso(&link.expires_at),
        "max_files": link.max_files,
        "files_used": link.files_used,
        "files": files,
        "attach_hint": "To send a file, pass attach [{\"type\": \"file\", \"id\": file_id}] to propose_customer_message.",
    })))
}

/// `revoke_upload_link`: the link takes no more files (its files stay).
pub async fn revoke(
    db: &mut PgConnection,
    user: &AuthUser,
    link_id: &str,
) -> ApiResult<Result<Value, String>> {
    let link = match own_link(db, user, link_id).await? {
        Ok(l) => l,
        Err(why) => return Ok(Err(why)),
    };
    sqlx::query("UPDATE upload_links SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(link.id)
        .execute(&mut *db)
        .await?;
    Ok(Ok(json!({
        "link_id": link.id.to_string(),
        "revoked": true,
        "files_used": link.files_used,
        "note": "The link takes no more files; the files it took stay where they are.",
    })))
}

// ─── The public side (token only) ───────────────────────────────────────────

/// The link a token opens, if any (looked up by hash, the same work whether or not it exists).
pub async fn by_token(db: &mut PgConnection, token: &str) -> ApiResult<Option<Link>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM upload_links WHERE token_hash = $1"
    ))
    .bind(token_hash(token))
    .fetch_optional(&mut *db)
    .await?)
}

fn not_found() -> ApiError {
    ApiError::http(404, "This upload link does not exist.")
}

/// The link of a token that still takes files: 404 for an unknown token, 410 (with the reason) for one that
/// is closed, expired or full.
pub async fn open_link(db: &mut PgConnection, token: &str) -> ApiResult<Link> {
    let link = by_token(db, token).await?.ok_or_else(not_found)?;
    if let Some(why) = closed_reason(&link, Utc::now()) {
        return Err(ApiError::http(410, why));
    }
    Ok(link)
}

/// `GET /public/drop/{token}`: what the drop page shows. 404 for an unknown token; a closed link still shows
/// (state says why), so the page can say so.
pub async fn public_view(db: &mut PgConnection, token: &str) -> ApiResult<Value> {
    let link = by_token(db, token).await?.ok_or_else(not_found)?;
    let caps = caps(db).await?.map_err(|why| ApiError::http(503, why))?;
    let now = Utc::now();
    let files: Vec<Value> = link_files(db, link.id)
        .await?
        .iter()
        .map(|f| {
            json!({"name": f.name, "size": f.size, "size_label": human_size(f.size),
                        "type": uploads::type_label(&f.mime)})
        })
        .collect();
    Ok(json!({
        "target_label": target_label(db, &link).await?,
        "state": state(&link, now),
        "reason": closed_reason(&link, now),
        "expires_at": iso(&link.expires_at),
        "max_files": link.max_files,
        "files_used": link.files_used,
        "files": files,
        "allowed": caps["allowed"],
        "accept": ".png,.jpg,.jpeg,.webp,.gif,.pdf,.docx,.xlsx,.mp4,.mov,.webm,image/png,image/jpeg,image/webp,image/gif,application/pdf,video/mp4,video/quicktime,video/webm",
        "file_max_bytes": caps["file_max_bytes"],
        "file_max_label": caps["file_max_label"],
        "video_max_bytes": caps["video_max_bytes"],
        "video_max_label": caps["video_max_label"],
    }))
}

/// How many bytes a PUT may send (the larger of the two caps); 503 naming the key when it is missing.
pub async fn read_cap(db: &mut PgConnection) -> ApiResult<i64> {
    uploads::read_cap(db)
        .await?
        .map_err(|why| ApiError::http(503, why))
}

/// The refusal of a body over the read cap.
pub fn too_large(max: i64) -> ApiError {
    ApiError::http(
        413,
        format!(
            "The file is over the {} one upload may be ({}; {} for videos).",
            human_size(max),
            uploads::MAX_KEY,
            uploads::VIDEO_MAX_KEY
        ),
    )
}

/// `PUT /public/drop/{token}/files`: stores one file on the link's target as its creator and counts it.
/// The link row is locked for the whole store, so max_files holds with uploads at once. Returns the file's
/// {file_id, file_kind, name, size, size_label, type, mime_type, duration_seconds, duration_label,
/// files_used, max_files}; the public route shows all but file_id and file_kind.
pub async fn accept_file(
    db: &mut PgConnection,
    token: &str,
    name: &str,
    data: Vec<u8>,
) -> ApiResult<Value> {
    let link: Link = sqlx::query_as(&format!(
        "SELECT {COLS} FROM upload_links WHERE token_hash = $1 FOR UPDATE"
    ))
    .bind(token_hash(token))
    .fetch_optional(&mut *db)
    .await?
    .ok_or_else(not_found)?;
    if let Some(why) = closed_reason(&link, Utc::now()) {
        return Err(ApiError::http(410, why));
    }
    if data.is_empty() {
        return Err(ApiError::http(400, "The file is empty."));
    }
    let sniffed = uploads::sniff(&data).map_err(|why| ApiError::http(415, why))?;
    let video = uploads::is_video(sniffed.mime);
    let (max, key) = if video {
        (uploads::video_max_bytes(db).await?, uploads::VIDEO_MAX_KEY)
    } else {
        (uploads::max_bytes(db).await?, uploads::MAX_KEY)
    };
    let max = max.map_err(|why| ApiError::http(503, why))?;
    if data.len() as i64 > max {
        return Err(ApiError::http(
            413,
            format!(
                "The {} is {}, over the {} one upload may be ({key}).",
                if video { "video" } else { "file" },
                human_size(data.len() as i64),
                human_size(max)
            ),
        ));
    }
    let Some(creator) = crate::nary::roles::load_actor(db, link.created_by_id).await? else {
        return Err(ApiError::http(
            410,
            "The person who made this upload link can no longer upload files.",
        ));
    };
    let size = data.len() as i64;
    let duration = if video {
        uploads::video_duration(&data)
    } else {
        None
    };
    let (file_id, kind, stored_name) = match link.entity_id {
        Some(eid) => {
            let out = uploads::store(
                db,
                &creator,
                &link.target,
                &eid.to_string(),
                name,
                data,
                UPLOAD_LINK_VIA,
            )
            .await?
            .map_err(|why| ApiError::http(403, why))?;
            let id = out["file_id"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| {
                    ApiError::internal(anyhow::anyhow!("upload store gave no file_id"))
                })?;
            let stored = out["file_name"].as_str().unwrap_or(name).to_string();
            (id, "attachment", stored)
        }
        None => {
            let clean = uploads::clean_name(name, &sniffed);
            let origin = Origin {
                source: "chat".into(),
                source_ref: format!("upload_link:{}:{}", link.id, Uuid::new_v4()),
                received_at: Some(Utc::now()),
                chat_user_id: Some(creator.id),
                ..Default::default()
            };
            let f = files::register_checked(
                db,
                Some(creator.id),
                &data,
                &clean,
                Some(sniffed.mime),
                &origin,
            )
            .await?;
            (f.id, "nary_file", clean)
        }
    };
    sqlx::query(
        "INSERT INTO upload_link_files (id, link_id, file_id, file_kind, name, size, mime, duration_seconds) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(link.id)
    .bind(file_id)
    .bind(kind)
    .bind(stored_name.chars().take(255).collect::<String>())
    .bind(size)
    .bind(sniffed.mime)
    .bind(duration)
    .execute(&mut *db)
    .await?;
    let used: i32 = sqlx::query_scalar(
        "UPDATE upload_links SET files_used = files_used + 1 WHERE id = $1 RETURNING files_used",
    )
    .bind(link.id)
    .fetch_one(&mut *db)
    .await?;
    tracing::info!(link_id = %link.id, file_id = %file_id, kind, size, mime = sniffed.mime,
                   "upload_link_file_stored");
    Ok(json!({
        "file_id": file_id.to_string(),
        "file_kind": kind,
        "name": stored_name,
        "size": size,
        "size_label": human_size(size),
        "type": sniffed.label,
        "mime_type": sniffed.mime,
        "duration_seconds": duration,
        "duration_label": duration.map(uploads::duration_label),
        "files_used": used,
        "max_files": link.max_files,
    }))
}

/// `POST /public/drop/{token}/close`: the page's Done. 404 for an unknown token; closing twice is fine.
pub async fn close(db: &mut PgConnection, token: &str) -> ApiResult<Value> {
    let link = by_token(db, token).await?.ok_or_else(not_found)?;
    sqlx::query("UPDATE upload_links SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(link.id)
        .execute(&mut *db)
        .await?;
    Ok(json!({"closed": true, "files_used": link.files_used}))
}

/// The public routes' limit: requests per minute per address and per link (files.upload_link.requests_per_minute,
/// 60). Fails open when Redis is down; a missing setting is a 503 naming it.
pub async fn rate_limit(db: &mut PgConnection, ip: Option<&str>, token: &str) -> ApiResult<()> {
    use redis::AsyncCommands;
    let limit = setting_int(db, RATE_KEY)
        .await?
        .map_err(|why| ApiError::http(503, why))?;
    let minute = Utc::now().format("%Y%m%d%H%M");
    let keys = [
        format!(
            "drop:rl:ip:{}:{minute}",
            ip.filter(|s| !s.is_empty()).unwrap_or("unknown")
        ),
        format!("drop:rl:tok:{}:{minute}", &token_hash(token)[..16]),
    ];
    for key in keys {
        let counted = async {
            let mut conn = granary_core::redis_client::get_redis().conn().await?;
            let count: i64 = conn.incr(&key, 1).await?;
            if count == 1 {
                let _: () = conn.expire(&key, 120).await?;
            }
            Ok::<i64, redis::RedisError>(count)
        }
        .await;
        if let Ok(count) = counted {
            if count > limit {
                return Err(ApiError::http(
                    429,
                    "Too many requests on this upload link: wait a minute and try again.",
                ));
            }
        }
    }
    Ok(())
}
