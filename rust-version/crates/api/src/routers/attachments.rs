//! backend/app/routers/attachments.py, mounted at `/api/v1/attachments` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (`InternalUser` plus
//! `services/attachments_access.assert_can_access`).
//!
//! Route order: Starlette registers `POST /upload` before `DELETE /{attachment_id}`, so `DELETE /upload`
//! reaches `delete_attachment("upload")` there; axum would answer 405 on the static route, so the static
//! route also carries that DELETE.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::{Cx, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::attachments_access::assert_can_access;
use granary_services::storage;

use super::at;
use crate::extract::PyForm;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/attachments";

/// The routes of `router` in attachments.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/upload"),
            post(upload_attachment).delete(delete_upload_path),
        )
        .route(
            &at(PREFIX, "/{attachment_id}/download"),
            get(download_attachment),
        )
        .route(&at(PREFIX, ""), get(list_attachments))
        .route(&at(PREFIX, "/{attachment_id}"), delete(delete_attachment))
}

pub const IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];
pub const VIDEO_TYPES: &[&str] = &["video/mp4", "video/webm", "video/ogg", "video/quicktime"];
/// Everything else that may be stored: an allowlist (the content type is replayed verbatim when served).
pub const DOCUMENT_TYPES: &[&str] = &[
    "application/pdf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "text/plain",
    "text/csv",
    "text/markdown",
    "application/zip",
    "application/x-zip-compressed",
    "application/json",
];
pub const MAX_IMAGE: usize = 5 * 1024 * 1024;
pub const MAX_VIDEO: usize = 100 * 1024 * 1024;
pub const MAX_DOC: usize = 25 * 1024 * 1024;

pub const ALLOWED_ENTITY_TYPES: &[&str] = &[
    "project",
    "rnd_project",
    "crm_org",
    "crm_lead",
    "crm_deal",
    "contract",
    "kb_article",
    // batch 4 D2: product files (datasheets, brochures) on catalog items; catalog managers add and remove them
    "catalog_product",
    // 29 Oct 2026: a contact's own files (the contact page; upload_file)
    "crm_contact",
];

fn allowed_type(mime: &str) -> bool {
    IMAGE_TYPES.contains(&mime) || VIDEO_TYPES.contains(&mime) || DOCUMENT_TYPES.contains(&mime)
}

/// The columns `_attachment_dict` reads.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: String,
    pub file_name: String,
    pub file_size: i32,
    pub mime_type: String,
    pub object_key: String,
    pub file_type: String,
    pub uploaded_by_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// `uploaded_by.name` when the query joins it.
    #[sqlx(default)]
    pub uploader_name: Option<String>,
    /// 29 Oct 2026 (migration 0141): 'mcp' or 'nary' for a file your AI client or Nary stored (upload_file,
    /// preview_visual save_to); None for a file uploaded on a page.
    #[sqlx(default)]
    pub uploaded_via: Option<String>,
}

const COLS: &str = "attachments.id, attachments.entity_type, attachments.entity_id, attachments.file_name, \
                    attachments.file_size, attachments.mime_type, attachments.object_key, attachments.file_type, \
                    attachments.uploaded_by_id, attachments.created_at, attachments.uploaded_via";

/// `_attachment_dict(a, uploader_name)`. `url` (the raw MINIO_PUBLIC_URL link) is deliberately absent.
pub fn attachment_dict(a: &AttachmentRow, uploader_name: Option<&str>) -> Value {
    let mut out = json!({
        "id": a.id.to_string(),
        "entity_type": a.entity_type,
        "entity_id": a.entity_id,
        "file_name": a.file_name,
        "file_size": a.file_size,
        "mime_type": a.mime_type,
        "file_type": a.file_type,
        "uploaded_by_id": a.uploaded_by_id.map(|u| u.to_string()),
        "uploaded_by_name": uploader_name,
        "created_at": resp::iso(&a.created_at),
    });
    // 29 Oct 2026: "via MCP" / "via Nary" in the list (only on such files, so the Python shape stays)
    if let Some(via) = a.uploaded_via.as_deref().filter(|v| !v.is_empty()) {
        out["uploaded_via"] = json!(via);
    }
    out
}

/// `os.path.basename(p)` (posix).
fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// `POST "/upload"` (201): a multipart file attached to a record the caller can see.
pub async fn upload_attachment(
    InternalUser(user): InternalUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let entity_type = form.req_text(&mut cx, "entity_type");
    let entity_id = form.req_text(&mut cx, "entity_id");
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let (entity_type, entity_id, file) = (entity_type.unwrap(), entity_id.unwrap(), file.unwrap());

    if !ALLOWED_ENTITY_TYPES.contains(&entity_type.as_str()) {
        return Err(ApiError::http(
            400,
            format!("Invalid entity_type: {entity_type}"),
        ));
    }
    // You may only attach to a record you can see.
    {
        let mut conn = db.conn().await?;
        assert_can_access(&mut conn, &user, &entity_type, &entity_id).await?;
        // batch 4 D2: only catalog managers add product files
        if entity_type == "catalog_product" {
            granary_core::auth::require_catalog_manager(&mut conn, &user).await?;
        }
    }

    let content = file.data;
    let size = content.len();
    let mime = file
        .content_type
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    if !allowed_type(&mime) {
        return Err(ApiError::http(
            415,
            format!("Files of type '{mime}' can't be uploaded here."),
        ));
    }
    let is_image = IMAGE_TYPES.contains(&mime.as_str());
    let is_video = VIDEO_TYPES.contains(&mime.as_str());
    if is_image && size > MAX_IMAGE {
        return Err(ApiError::http(413, "Image exceeds 5 MB limit"));
    } else if is_video && size > MAX_VIDEO {
        return Err(ApiError::http(413, "Video exceeds 100 MB limit"));
    } else if !is_image && !is_video && size > MAX_DOC {
        return Err(ApiError::http(413, "Document exceeds 25 MB limit"));
    }
    let file_type = if is_image {
        "image"
    } else if is_video {
        "video"
    } else {
        "file"
    };

    let filename = if file.filename.is_empty() {
        "upload"
    } else {
        file.filename.as_str()
    };
    let safe_name = basename(filename).to_string();
    let object_key = format!(
        "attachments/{entity_type}/{entity_id}/{}/{safe_name}",
        Uuid::new_v4()
    );
    let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();

    let store = storage::minio();
    storage::ensure_bucket(store.as_ref(), &bucket)
        .await
        .map_err(ApiError::internal)?;
    store
        .put_object(&bucket, &object_key, &content, &mime)
        .await
        .map_err(ApiError::internal)?;

    // security 26 Oct: the bucket is private; the file is read through /download, no link is kept
    let url = String::new();
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    let row: AttachmentRow = sqlx::query_as(&format!(
        "INSERT INTO attachments (entity_type, entity_id, uploaded_by_id, file_name, file_size, mime_type, \
         object_key, url, file_type, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLS}"
    ))
    .bind(&entity_type)
    .bind(&entity_id)
    .bind(user.id)
    .bind(&safe_name)
    .bind(size as i64)
    .bind(&mime)
    .bind(&object_key)
    .bind(&url)
    .bind(file_type)
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        attachment_dict(&row, Some(&user.name)),
    ))
}

/// `GET "/{attachment_id}/download"`: authenticated proxy that pulls the object from MinIO.
pub async fn download_attachment(
    InternalUser(user): InternalUser,
    db: Db,
    Path(attachment_id): Path<String>,
) -> ApiResult<Response> {
    let id = pg_uuid(&attachment_id)?;
    let mut conn = db.conn().await?;
    let row: Option<AttachmentRow> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM attachments WHERE attachments.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(a) = row else {
        return Err(ApiError::http(404, "Attachment not found"));
    };
    assert_can_access(&mut conn, &user, &a.entity_type, &a.entity_id).await?;
    drop(conn);

    // batch 4 D1: Nary's customer files are read from the private documents bucket
    let bucket = storage::attachment_bucket(&a.object_key);
    let data = storage::minio()
        .get_object(&bucket, &a.object_key)
        .await
        .map_err(|_| ApiError::http(404, "File not found in storage"))?;

    // Inline only for types a browser renders as content and still allowed at upload; a Nary customer
    // file keeps the sender's type, which the shared helper downgrades when it could run as a page.
    let inline = allowed_type(&a.mime_type);
    let filename = if a.file_name.is_empty() {
        "download"
    } else {
        a.file_name.as_str()
    };
    let mut headers = vec![("cache-control", "private, max-age=300".to_string())];
    // Python sends Content-Length = file_size (when set) on a streamed body; here the body is in memory
    // and its own length is sent, which is the same number for an intact object.
    if a.file_size != 0 && a.file_size as usize == data.len() {
        headers.push(("content-length", a.file_size.to_string()));
    }
    Ok(resp::with_headers(
        crate::files::safe_file_response(data, &a.mime_type, filename, inline),
        &headers,
    ))
}

granary_core::py_model! {
    /// The query of `list_attachments`.
    pub struct ListQ {
        pub entity_type: String,
        pub entity_id: String,
    }
}

/// `GET ""`: the files on one record, newest first.
pub async fn list_attachments(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ListQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    assert_can_access(&mut conn, &user, &q.entity_type, &q.entity_id).await?;
    // selectinload(Attachment.uploaded_by): the uploader's name (a LEFT JOIN gives the same rows).
    let rows: Vec<(AttachmentRow, Option<String>)> = sqlx::query_as::<_, AttachmentRow>(&format!(
        "SELECT {COLS}, users.name AS uploader_name FROM attachments \
         LEFT OUTER JOIN users ON users.id = attachments.uploaded_by_id \
         WHERE attachments.entity_type = $1 AND attachments.entity_id = $2 \
         ORDER BY attachments.created_at DESC"
    ))
    .bind(&q.entity_type)
    .bind(&q.entity_id)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|mut a| {
        let name = a.uploader_name.take();
        (a, name)
    })
    .collect();
    // doclib A1: product files moved into the document library are listed there, not here
    let moved: Vec<String> = if q.entity_type == "catalog_product" {
        granary_services::doc_library::migrate_product_files::migrated_keys(&mut conn, &q.entity_id)
            .await?
    } else {
        vec![]
    };
    let out: Vec<Value> = rows
        .iter()
        .filter(|(a, _)| !moved.contains(&a.object_key))
        .map(|(a, name)| attachment_dict(a, name.as_deref()))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `DELETE "/{attachment_id}"` (204): the uploader or an admin.
pub async fn delete_attachment(
    InternalUser(user): InternalUser,
    db: Db,
    Path(attachment_id): Path<String>,
) -> ApiResult<Response> {
    delete_one(user, db, &attachment_id).await
}

/// `DELETE "/upload"`: Starlette hands it to `delete_attachment("upload")` (see the module doc).
pub async fn delete_upload_path(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    delete_one(user, db, "upload").await
}

async fn delete_one(
    user: granary_core::auth::AuthUser,
    db: Db,
    attachment_id: &str,
) -> ApiResult<Response> {
    let id = pg_uuid(attachment_id)?;
    let mut conn = db.conn().await?;
    let row: Option<AttachmentRow> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM attachments WHERE attachments.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(a) = row else {
        return Err(ApiError::http(404, "Attachment not found"));
    };
    assert_can_access(&mut conn, &user, &a.entity_type, &a.entity_id).await?;
    // batch 4 D2: only catalog managers remove product files
    if a.entity_type == "catalog_product" {
        granary_core::auth::require_catalog_manager(&mut conn, &user).await?;
    }

    let is_owner = a.uploaded_by_id == Some(user.id);
    if !is_owner && !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(
            403,
            "You can only delete your own attachments",
        ));
    }
    // 5 Nov 2026 (store once): a document library version may use this very object (a product file added
    // from the record); the object then stays, only the listing goes
    let in_library =
        granary_services::doc_library::references_object(&mut conn, &a.object_key).await?;
    drop(conn);

    // Don't block DB cleanup if the MinIO object is already gone.
    // batch 4 D1: a Nary customer file's object is shared by every record it is listed on (and by Nary's own
    // copy), so only the listing goes.
    if !in_library && !a.object_key.starts_with(storage::NARY_FILES_PREFIX) {
        let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
        let _ = storage::minio().remove_object(&bucket, &a.object_key).await;
    }

    let mut conn = db.conn().await?;
    sqlx::query("DELETE FROM attachments WHERE attachments.id = $1")
        .bind(a.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
