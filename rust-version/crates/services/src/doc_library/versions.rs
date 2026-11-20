//! Documents and their versions: create, upload a version, go live, reject, restore.
//!
//! - The sent file is a PDF or an image (png/jpg/webp/gif); a DOCX/XLSX/PPTX may ride along as the editable
//!   source (never sent) and then the file must be the PDF of it. Both at most documents.max_file_mb.
//! - Files are stored once per content in the private documents bucket (`doclib/<sha256>`,
//!   `doclib/src/<sha256>`); a restored version points at the same objects.
//! - The text is read on upload (batch 4 nary::files::units_with: PDF text layer, scanned pages and images by
//!   the vision model).
//! - 5 Nov 2026: a version may also be a video (MP4, MOV, WebM), judged by its bytes (nary::uploads::sniff),
//!   at most nary.upload.video_max_mb; its length is kept (duration_seconds), no text is read, and its copies
//!   are logged but never watermarked (fingerprint::stamped_copy). A version may reference a CRM attachment's
//!   own object instead of a copy (store once: `file_bucket` assets, see [`StoredFile`]).
//! - Internal-only documents go live at once (the live version before becomes superseded). A document with a
//!   customer or partner audience goes through the approval engine (adapter library_document_version):
//!   pending_approval until a director approves (live) or rejects (rejected, with the reason). The Owner's own
//!   upload is approved on the spot. A new upload while one is pending replaces it (the pending one becomes
//!   rejected, "Replaced by ...").

use chrono::NaiveDate;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::config::settings;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;

use super::{access, Document, Version, AUDIENCES, OBJECT_PREFIX, VERSION_COLS, WHERE};
use crate::approvals::engine;
use crate::nary::file_text;
use crate::nary::files as nary_files;
use crate::nary::uploads;
use crate::storage::{ensure_bucket, minio};

/// The approval engine's entity type of a version.
pub const ENTITY_TYPE: &str = "library_document_version";

/// Image types a version may be.
pub const IMAGE_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];
/// Editable source types (never sent).
pub const SOURCE_EXTS: [&str; 3] = ["docx", "xlsx", "pptx"];
/// 5 Nov 2026: video types a version may be (judged by the bytes).
pub const VIDEO_MIMES: [&str; 3] = ["video/mp4", "video/quicktime", "video/webm"];
const VIDEO_EXTS: [&str; 4] = ["mp4", "m4v", "mov", "webm"];

/// An uploaded file.
#[derive(Debug, Clone)]
pub struct NewFile {
    pub data: Vec<u8>,
    pub file_name: String,
    pub mime: Option<String>,
}

/// A product a document belongs to, or a whole family.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProductRef {
    #[serde(default)]
    pub product_id: Option<Uuid>,
    #[serde(default)]
    pub family: Option<String>,
}

/// The details of a new document (the `meta` JSON of POST /knowledge/documents).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewDocument {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// a category key (company, products, ...) or id
    pub category: String,
    pub audiences: Vec<String>,
    #[serde(default)]
    pub owner_id: Option<Uuid>,
    #[serde(default)]
    pub language: Option<String>,
    /// another document this one is a language variant of (joins its group)
    #[serde(default, alias = "language_group_document_id")]
    pub variant_of: Option<Uuid>,
    #[serde(default)]
    pub review_by: Option<NaiveDate>,
    #[serde(default)]
    pub products: Vec<ProductRef>,
    /// the first version's label (default v1)
    #[serde(default)]
    pub label: Option<String>,
    /// the first version's change note (default "First version")
    #[serde(default)]
    pub change_note: Option<String>,
}

/// A new version (POST /knowledge/documents/{id}/versions).
#[derive(Debug, Clone)]
pub struct NewVersion {
    pub label: Option<String>,
    pub change_note: String,
    pub file: NewFile,
    pub source: Option<NewFile>,
}

fn ext_of(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.rsplit_once('.') {
        Some((_, e)) => e.to_string(),
        None => String::new(),
    }
}

fn clean_name(name: &str) -> String {
    let n = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let n = if n.is_empty() { "file" } else { n };
    n.chars().take(255).collect()
}

fn clean_mime(mime: Option<&str>, name: &str) -> String {
    let m = mime
        .map(|m| m.split(';').next().unwrap_or("").trim().to_lowercase())
        .filter(|m| !m.is_empty() && m != "application/octet-stream");
    if let Some(m) = m {
        return m;
    }
    match ext_of(name).as_str() {
        "pdf" => "application/pdf".into(),
        "png" => "image/png".into(),
        "jpg" | "jpeg" => "image/jpeg".into(),
        "webp" => "image/webp".into(),
        "gif" => "image/gif".into(),
        // 5 Nov 2026: videos (the bytes decide in check_files)
        "mp4" | "m4v" => "video/mp4".into(),
        "mov" => "video/quicktime".into(),
        "webm" => "video/webm".into(),
        "docx" => file_text::DOCX_MIME.into(),
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
        "pptx" => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation".into()
        }
        _ => "application/octet-stream".into(),
    }
}

/// pdf | image | None (what a version's sent file may be, by its name and type; a video is judged by its bytes
/// in check_files).
pub fn sendable_kind(name: &str, mime: &str) -> Option<&'static str> {
    if mime == "application/pdf" || ext_of(name) == "pdf" {
        return Some("pdf");
    }
    if IMAGE_MIMES.contains(&mime) {
        return Some("image");
    }
    None
}

/// The size cap in bytes (documents.max_file_mb).
pub async fn max_bytes(db: &mut PgConnection) -> ApiResult<i64> {
    Ok(require_int(db, "documents.max_file_mb", WHERE).await? * 1024 * 1024)
}

fn bucket() -> String {
    settings().MINIO_BUCKET_DOCUMENTS.clone()
}

/// Store bytes once per content; the key.
async fn store(prefix: &str, data: &[u8], mime: &str) -> ApiResult<(String, String)> {
    let sha = hex::encode(Sha256::digest(data));
    let key = format!("{OBJECT_PREFIX}{prefix}{sha}");
    let s = minio();
    let b = bucket();
    ensure_bucket(&*s, &b).await.map_err(ApiError::internal)?;
    s.put_object(&b, &key, data, mime)
        .await
        .map_err(ApiError::internal)?;
    Ok((key, sha))
}

/// The checked file and source: (file name, mime, kind) and the source's (name, mime).
async fn check_files(
    db: &mut PgConnection,
    file: &NewFile,
    source: Option<&NewFile>,
) -> ApiResult<(String, String, &'static str, Option<(String, String)>)> {
    let max = max_bytes(db).await?;
    let too_big = |n: usize| n as i64 > max;
    let name = clean_name(&file.file_name);
    let mime = clean_mime(file.mime.as_deref(), &name);
    if file.data.is_empty() {
        return Err(ApiError::http(400, "The file is empty"));
    }
    // 5 Nov 2026: a video is judged by its bytes (MP4, MOV, WebM), never by its name or claimed type
    let sniffed = uploads::sniff(&file.data);
    if let Ok(sn) = sniffed.as_ref() {
        if uploads::is_video(sn.mime) {
            if source.is_some() {
                return Err(ApiError::http(
                    400,
                    "A video can't have an editable source file",
                ));
            }
            let cap = match uploads::video_max_bytes(db).await? {
                Ok(c) => c,
                Err(why) => return Err(ApiError::http(400, why)),
            };
            if file.data.len() as i64 > cap {
                return Err(ApiError::http(
                    413,
                    format!(
                        "The video is larger than {} MB (setting {})",
                        cap / 1024 / 1024,
                        uploads::VIDEO_MAX_KEY
                    ),
                ));
            }
            return Ok((
                uploads::clean_name(&name, sn),
                sn.mime.to_string(),
                "video",
                None,
            ));
        }
    }
    if uploads::is_video(&mime) || VIDEO_EXTS.contains(&ext_of(&name).as_str()) {
        let why = match sniffed {
            Err(why) => why,
            Ok(sn) => format!("This file is a {}, not a video", sn.label),
        };
        return Err(ApiError::http(
            415,
            format!("{why} (a video version must be an MP4, MOV or WebM file)"),
        ));
    }
    let src = match source {
        None => None,
        Some(s) => {
            let sname = clean_name(&s.file_name);
            if !SOURCE_EXTS.contains(&ext_of(&sname).as_str()) {
                return Err(ApiError::http(
                    400,
                    "The editable source must be a Word, Excel or PowerPoint file (docx, xlsx, pptx)",
                ));
            }
            if s.data.is_empty() {
                return Err(ApiError::http(400, "The source file is empty"));
            }
            if too_big(s.data.len()) {
                return Err(ApiError::http(
                    413,
                    format!(
                        "The source file is larger than {} MB (setting documents.max_file_mb)",
                        max / 1024 / 1024
                    ),
                ));
            }
            let smime = clean_mime(s.mime.as_deref(), &sname);
            Some((sname, smime))
        }
    };
    let Some(kind) = sendable_kind(&name, &mime) else {
        if SOURCE_EXTS.contains(&ext_of(&name).as_str()) {
            return Err(ApiError::http(
                400,
                "Upload the PDF of this file; the Word, Excel or PowerPoint file can go along as the editable source",
            ));
        }
        return Err(ApiError::http(
            415,
            "A document version must be a PDF, an image (PNG, JPG, WebP, GIF) or a video (MP4, MOV, WebM)",
        ));
    };
    if src.is_some() && kind != "pdf" {
        return Err(ApiError::http(
            400,
            "A version with an editable source must be the PDF of it",
        ));
    }
    if too_big(file.data.len()) {
        return Err(ApiError::http(
            413,
            format!(
                "The file is larger than {} MB (setting documents.max_file_mb)",
                max / 1024 / 1024
            ),
        ));
    }
    Ok((name, mime, kind, src))
}

/// 5 Nov 2026: the checks of an upload's file alone (the product file card checks before it is made): the
/// file name kept, its type and kind (pdf | image | video).
pub async fn check_file(
    db: &mut PgConnection,
    file: &NewFile,
) -> ApiResult<(String, String, &'static str)> {
    let (name, mime, kind, _) = check_files(db, file, None).await?;
    Ok((name, mime, kind))
}

fn clean_audiences(aud: &[String]) -> ApiResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for a in aud {
        let a = a.trim().to_lowercase();
        if !AUDIENCES.contains(&a.as_str()) {
            return Err(ApiError::http(
                400,
                format!("Unknown audience '{a}': use internal, customer or partner"),
            ));
        }
        if !out.contains(&a) {
            out.push(a);
        }
    }
    if out.is_empty() {
        return Err(ApiError::http(
            400,
            "A document needs at least one audience (internal, customer or partner)",
        ));
    }
    // a stable order: internal, customer, partner
    out.sort_by_key(|a| AUDIENCES.iter().position(|x| x == a));
    Ok(out)
}

pub fn clean_language(l: Option<&str>) -> ApiResult<String> {
    let l = l.map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if l.is_empty() {
        return Ok("en".into());
    }
    if l.len() != 2 || !l.chars().all(|c| c.is_ascii_lowercase()) {
        return Err(ApiError::http(
            400,
            "The language is a two-letter ISO 639-1 code (en, fr, ...)",
        ));
    }
    Ok(l)
}

/// The id of a category from its key or id (400 when unknown).
pub async fn category_id(db: &mut PgConnection, key_or_id: &str) -> ApiResult<Uuid> {
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM doc_categories WHERE key = $1 OR id::text = $1 LIMIT 1")
            .bind(key_or_id.trim())
            .fetch_optional(&mut *db)
            .await?;
    id.ok_or_else(|| ApiError::http(400, format!("Unknown category '{key_or_id}'")))
}

/// Replace a document's products.
pub async fn set_products(
    db: &mut PgConnection,
    document_id: Uuid,
    products: &[ProductRef],
) -> ApiResult<()> {
    for p in products {
        match (&p.product_id, p.family.as_deref().map(str::trim)) {
            (Some(pid), None) | (Some(pid), Some("")) => {
                let ok: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM product_catalog WHERE id = $1 AND is_deleted = false",
                )
                .bind(pid)
                .fetch_optional(&mut *db)
                .await?;
                if ok.is_none() {
                    return Err(ApiError::http(400, format!("Unknown product {pid}")));
                }
            }
            (None, Some(f)) if !f.is_empty() => {}
            _ => {
                return Err(ApiError::http(
                    400,
                    "Each product entry is a product_id or a family (one of them)",
                ))
            }
        }
    }
    sqlx::query("DELETE FROM library_document_products WHERE document_id = $1")
        .bind(document_id)
        .execute(&mut *db)
        .await?;
    for p in products {
        let fam = p
            .family
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty() && p.product_id.is_none())
            .map(|f| f.chars().take(50).collect::<String>());
        sqlx::query(
            "INSERT INTO library_document_products (id, document_id, product_id, family) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(document_id)
        .bind(p.product_id)
        .bind(fam)
        .execute(&mut *db)
        .await?;
    }
    Ok(())
}

/// The language group a document joins as a variant of `other` (creates the group on `other` if needed).
pub async fn group_with(db: &mut PgConnection, other: Uuid) -> ApiResult<Uuid> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT language_group_id FROM library_documents WHERE id = $1 AND is_deleted = false",
    )
    .bind(other)
    .fetch_optional(&mut *db)
    .await?;
    let Some((group,)) = row else {
        return Err(ApiError::http(
            400,
            "The other language version was not found",
        ));
    };
    if let Some(g) = group {
        return Ok(g);
    }
    let g = Uuid::new_v4();
    sqlx::query(
        "UPDATE library_documents SET language_group_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(other)
    .bind(g)
    .execute(&mut *db)
    .await?;
    Ok(g)
}

/// 5 Nov 2026: how a version comes in besides the plain upload.
#[derive(Debug, Clone, Default)]
pub struct VersionOpts {
    /// store once: the file is already stored here (its bytes are still given, to check and read them)
    pub stored: Option<StoredFile>,
    /// applied from a card the Owner confirmed (catalog.product_file): the rights were checked by the card, and
    /// a customer or partner version goes live with this approver (the card is the approval)
    pub card_approver: Option<Uuid>,
}

/// 5 Nov 2026 (store once): an object already in storage that a version references instead of a copy.
#[derive(Debug, Clone)]
pub struct StoredFile {
    /// `file_bucket`: [`super::BUCKET_DOCUMENTS`] or [`super::BUCKET_ASSETS`]
    pub bucket: &'static str,
    pub key: String,
}

/// Create a document and its first version (POST /knowledge/documents). documents:create.
pub async fn create_document(
    db: &mut PgConnection,
    user: &AuthUser,
    meta: NewDocument,
    file: NewFile,
    source: Option<NewFile>,
) -> ApiResult<(Document, Version)> {
    create_document_with(db, user, meta, file, source, VersionOpts::default()).await
}

/// [`create_document`] with the options of a card (store once, the Owner's card as the approval).
pub async fn create_document_with(
    db: &mut PgConnection,
    user: &AuthUser,
    meta: NewDocument,
    file: NewFile,
    source: Option<NewFile>,
    opts: VersionOpts,
) -> ApiResult<(Document, Version)> {
    let by_card = opts.card_approver.is_some();
    if !by_card && !access::can_create(user) {
        return Err(ApiError::http(403, "Permission required: documents:create"));
    }
    let title: String = meta.title.trim().chars().take(300).collect();
    if title.is_empty() {
        return Err(ApiError::http(400, "A document needs a title"));
    }
    let audiences = clean_audiences(&meta.audiences)?;
    let language = clean_language(meta.language.as_deref())?;
    let category_id = category_id(db, &meta.category).await?;
    if !by_card && !access::can_view_category(db, user, category_id).await? {
        return Err(ApiError::http(403, "You cannot see this category"));
    }
    // the owner is the creator unless a director or doc admin names someone else
    let owner = match meta.owner_id {
        Some(o) if o != user.id => {
            if !by_card && !access::is_doc_admin(user) {
                return Err(ApiError::http(
                    403,
                    "Only directors and document admins can make someone else the owner",
                ));
            }
            o
        }
        _ => user.id,
    };
    check_files(db, &file, source.as_ref()).await?;
    let group = match meta.variant_of {
        Some(other) => Some(group_with(db, other).await?),
        None => None,
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO library_documents (id, title, description, category_id, audiences, owner_id, language, \
         language_group_id, review_by, kind, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'uploaded', $10)",
    )
    .bind(id)
    .bind(&title)
    .bind(meta.description.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(category_id)
    .bind(&audiences)
    .bind(owner)
    .bind(&language)
    .bind(group)
    .bind(meta.review_by)
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    set_products(db, id, &meta.products).await?;
    let doc = super::load_document(db, id)
        .await?
        .ok_or_else(|| ApiError::http(500, "Document vanished"))?;
    let v = add_version_with(
        db,
        user,
        &doc,
        NewVersion {
            label: meta.label,
            change_note: meta
                .change_note
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "First version".into()),
            file,
            source,
        },
        opts,
    )
    .await?;
    let doc = super::load_document(db, id)
        .await?
        .ok_or_else(|| ApiError::http(500, "Document vanished"))?;
    Ok((doc, v))
}

/// Upload a new version (the owner with documents:create, or a director). Returns the version as it is now
/// (live, or pending_approval).
pub async fn add_version(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: &Document,
    new: NewVersion,
) -> ApiResult<Version> {
    add_version_with(db, user, doc, new, VersionOpts::default()).await
}

/// [`add_version`] with the options of a card (store once, the Owner's card as the approval).
pub async fn add_version_with(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: &Document,
    new: NewVersion,
    opts: VersionOpts,
) -> ApiResult<Version> {
    if opts.card_approver.is_none() && !access::can_edit(db, user, doc).await? {
        return Err(ApiError::http(
            403,
            "Only the document's owner or a director can upload a version",
        ));
    }
    let (name, mime, kind, src) = check_files(db, &new.file, new.source.as_ref()).await?;
    // 5 Nov 2026 (store once): an object already stored is referenced, not copied
    let (key, sha, bucket) = match &opts.stored {
        Some(st) => (
            st.key.clone(),
            hex::encode(Sha256::digest(&new.file.data)),
            st.bucket,
        ),
        None => {
            let (k, sh) = store("", &new.file.data, &mime).await?;
            (k, sh, super::BUCKET_DOCUMENTS)
        }
    };
    let source = match (&new.source, src) {
        (Some(s), Some((sname, smime))) => {
            let (skey, _) = store("src/", &s.data, &smime).await?;
            Some((skey, sname, smime, s.data.len() as i64))
        }
        _ => None,
    };
    // the text, once (vision only for scanned pages and images); 5 Nov 2026: a video has none, only its length
    let (units, pages, duration) = if kind == "video" {
        (None, None, uploads::video_duration(&new.file.data))
    } else {
        let caps = nary_files::caps(db).await?;
        let read =
            nary_files::units_with(db, &new.file.data, &name, &mime, kind, Some(user.id), caps)
                .await;
        match read {
            Ok((u, p, _)) => (Some(file_text::units_json(&u)), p, None),
            Err(_) => (None, if kind == "image" { Some(1) } else { None }, None),
        }
    };
    insert_version(
        db,
        user,
        doc,
        InsertVersion {
            label: new.label,
            change_note: new.change_note,
            key,
            name,
            mime,
            size: new.file.data.len() as i64,
            sha,
            pages,
            source,
            units,
            duration,
            bucket: bucket.to_string(),
            card_approver: opts.card_approver,
        },
    )
    .await
}

/// What a version row is made of (an upload, a restore, a migrated product file, the price list).
pub struct InsertVersion {
    pub label: Option<String>,
    pub change_note: String,
    pub key: String,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub sha: String,
    pub pages: Option<i32>,
    /// (object key, file name, mime, size)
    pub source: Option<(String, String, String, i64)>,
    pub units: Option<serde_json::Value>,
    /// 5 Nov 2026: a video's length in seconds
    pub duration: Option<f64>,
    /// 5 Nov 2026: `file_bucket` (documents, or assets for a referenced CRM attachment)
    pub bucket: String,
    /// 5 Nov 2026: applied from the Owner's card: an external version goes live with this approver
    pub card_approver: Option<Uuid>,
}

/// Insert a version and route it (live at once, or through approval). The caller checked the rights.
pub async fn insert_version(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: &Document,
    v: InsertVersion,
) -> ApiResult<Version> {
    let note = v.change_note.trim().to_string();
    if note.is_empty() {
        return Err(ApiError::http(
            400,
            "Say what changed in this version (change note)",
        ));
    }
    let label = match v.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => l.chars().take(60).collect::<String>(),
        None => super::next_label(db, doc.id).await?,
    };
    let taken: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM library_document_versions WHERE document_id = $1 AND label = $2",
    )
    .bind(doc.id)
    .bind(&label)
    .fetch_optional(&mut *db)
    .await?;
    if taken.is_some() {
        return Err(ApiError::http(
            409,
            format!("This document already has a version {label}"),
        ));
    }
    // a version still waiting for approval is replaced by this one
    let pending: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, label FROM library_document_versions WHERE document_id = $1 AND status = 'pending_approval'",
    )
    .bind(doc.id)
    .fetch_all(&mut *db)
    .await?;
    for (pid, _) in &pending {
        engine::supersede_for_entity(db, ENTITY_TYPE, *pid, &format!("Replaced by {label}"))
            .await?;
        sqlx::query(
            "UPDATE library_document_versions SET status = 'rejected', rejected_reason = $2 WHERE id = $1",
        )
        .bind(pid)
        .bind(format!("Replaced by {label} before approval"))
        .execute(&mut *db)
        .await?;
    }
    let id = Uuid::new_v4();
    let (skey, sname, smime, ssize) = match v.source {
        Some((a, b, c, d)) => (Some(a), Some(b), Some(c), Some(d)),
        None => (None, None, None, None),
    };
    sqlx::query(
        "INSERT INTO library_document_versions (id, document_id, label, change_note, status, file_object_key, \
         file_name, file_mime, file_size, file_sha256, page_count, source_object_key, source_file_name, source_mime, \
         source_size, extracted_units, extracted_at, uploaded_by, duration_seconds, file_bucket) \
         VALUES ($1, $2, $3, $4, 'draft', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, \
                 CASE WHEN $15::jsonb IS NULL THEN NULL ELSE now() END, $16, $17, $18)",
    )
    .bind(id)
    .bind(doc.id)
    .bind(&label)
    .bind(&note)
    .bind(&v.key)
    .bind(&v.name)
    .bind(v.mime.chars().take(120).collect::<String>())
    .bind(v.size)
    .bind(&v.sha)
    .bind(v.pages)
    .bind(skey)
    .bind(sname)
    .bind(smime.map(|m| m.chars().take(160).collect::<String>()))
    .bind(ssize)
    .bind(v.units)
    .bind(user.id)
    .bind(v.duration)
    .bind(&v.bucket)
    .execute(&mut *db)
    .await?;
    sqlx::query("UPDATE library_documents SET updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    match v.card_approver {
        // 5 Nov 2026: the Owner's confirmed card is the approval (recorded as theirs, like their own upload)
        Some(approver) => {
            go_live(
                db,
                id,
                if doc.is_external() {
                    Some(approver)
                } else {
                    None
                },
            )
            .await?
        }
        None => route(db, user, doc, id).await?,
    }
    super::load_version(db, id)
        .await?
        .ok_or_else(|| ApiError::http(500, "Version vanished"))
}

/// Internal-only: live at once. Customer/partner: submitted for approval (no active policy: live at once;
/// the Owner's own upload is approved on the spot).
async fn route(db: &mut PgConnection, user: &AuthUser, doc: &Document, vid: Uuid) -> ApiResult<()> {
    if !doc.is_external() {
        return go_live(db, vid, None).await;
    }
    match engine::submit_if_policy(db, ENTITY_TYPE, &vid.to_string(), user, None).await? {
        None => go_live(db, vid, None).await,
        Some(req) => {
            if req.status == "pending" && user.is_superadmin {
                engine::act_on_pending(db, ENTITY_TYPE, vid, user, "approved", None).await?;
            }
            Ok(())
        }
    }
}

/// Make a version the live one: the live version before becomes superseded; the staff who can see the
/// category hear about it. `approver` is recorded for approved versions.
pub async fn go_live(
    db: &mut PgConnection,
    version_id: Uuid,
    approver: Option<Uuid>,
) -> ApiResult<()> {
    let Some(v) = super::load_version(db, version_id).await? else {
        return Err(ApiError::http(404, "Version not found"));
    };
    if v.status == super::STATUS_LIVE {
        return Ok(());
    }
    sqlx::query(
        "UPDATE library_document_versions SET status = 'superseded' \
         WHERE document_id = $1 AND status = 'live' AND id <> $2",
    )
    .bind(v.document_id)
    .bind(v.id)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "UPDATE library_document_versions SET status = 'live', went_live_at = now(), rejected_reason = NULL, \
         approved_by = COALESCE($2, approved_by), approved_at = CASE WHEN $2::uuid IS NULL THEN approved_at ELSE now() END \
         WHERE id = $1",
    )
    .bind(v.id)
    .bind(approver)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "UPDATE library_documents SET current_version_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(v.document_id)
    .bind(v.id)
    .execute(&mut *db)
    .await?;
    if let (Some(doc), Some(v)) = (
        super::load_document(db, v.document_id).await?,
        super::load_version(db, v.id).await?,
    ) {
        super::notify::new_live_version(db, &doc, &v).await?;
    }
    Ok(())
}

/// A version an approver rejected (or that was withdrawn): rejected with the reason.
pub async fn mark_rejected(
    db: &mut PgConnection,
    version_id: Uuid,
    reason: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE library_document_versions SET status = 'rejected', rejected_reason = $2 \
         WHERE id = $1 AND status IN ('draft', 'pending_approval')",
    )
    .bind(version_id)
    .bind(
        reason
            .filter(|r| !r.trim().is_empty())
            .unwrap_or("Rejected"),
    )
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Restore an old version (doc admins): a new version with the same files, the suggested label and the note
/// "Restored from vX"; same approval rules as an upload.
pub async fn restore(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: &Document,
    version_id: Uuid,
) -> ApiResult<Version> {
    access::require_doc_admin(user)?;
    let Some(old) = super::load_version(db, version_id).await? else {
        return Err(ApiError::http(404, "Version not found"));
    };
    if old.document_id != doc.id {
        return Err(ApiError::http(404, "Version not found"));
    }
    let source = match (old.source_object_key, old.source_file_name, old.source_mime) {
        (Some(k), Some(n), Some(m)) => Some((k, n, m, old.source_size.unwrap_or(0))),
        _ => None,
    };
    insert_version(
        db,
        user,
        doc,
        InsertVersion {
            label: None,
            change_note: format!("Restored from {}", old.label),
            key: old.file_object_key,
            name: old.file_name,
            mime: old.file_mime,
            size: old.file_size,
            sha: old.file_sha256,
            pages: old.page_count,
            source,
            units: old.extracted_units,
            duration: old.duration_seconds,
            bucket: old.file_bucket,
            card_approver: None,
        },
    )
    .await
}

/// The versions of a document waiting for approval (any document), newest first.
pub async fn pending_versions(db: &mut PgConnection) -> ApiResult<Vec<Version>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {VERSION_COLS} FROM library_document_versions WHERE status = 'pending_approval' \
         ORDER BY created_at DESC"
    ))
    .fetch_all(&mut *db)
    .await?)
}
