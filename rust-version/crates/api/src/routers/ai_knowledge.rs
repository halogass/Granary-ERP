//! backend/app/routers/ai_knowledge.py, mounted at `/api/v1/ai/knowledge` (backend/app/main.py).
//!
//! AI knowledge, admin only:
//!
//!   GET    ""                 list documents (no content)
//!   POST   ""                 upload a .md / .pdf, or paste Markdown: converted and indexed on a worker
//!   GET    /{id}              one document, with the Markdown the AI reads
//!   PATCH  /{id}              title, description, on/off, or (Markdown) the content: re-indexed
//!   POST   /{id}/reindex      rebuild its embeddings
//!   DELETE /{id}              remove it, its chunks and the stored file
//!   POST   /search            "what would the AI find for this question?"
//!
//! Only superadmins and system admins get past `_require_admin`. The originals live in the private
//! documents bucket.
//!
//! Starlette quirk kept: `/{doc_id}` is registered before `POST /search`, so GET, PATCH and DELETE on
//! `/search` reach the document handlers with doc_id "search" (404 "Document not found"); axum would
//! answer 405, so `/search` also routes those methods to them.
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`.

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{v, PyJson};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::ai::AIKnowledgeDoc;
use granary_services::ai::knowledge;

use super::at;
use crate::extract::PyForm;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/ai/knowledge";

pub const MARKDOWN_EXT: [&str; 3] = [".md", ".markdown", ".txt"];
pub const MARKDOWN_MAX_BYTES: usize = 5 * 1024 * 1024;

/// The routes of `router` in ai_knowledge.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, ""), get(list_documents).post(add_document))
        .route(
            &at(PREFIX, "/search"),
            post(test_search)
                .get(get_search_as_document)
                .patch(update_search_as_document)
                .delete(delete_search_as_document),
        )
        .route(
            &at(PREFIX, "/{doc_id}"),
            get(get_document)
                .patch(update_document)
                .delete(delete_document),
        )
        .route(&at(PREFIX, "/{doc_id}/reindex"), post(reindex_document))
}

fn _require_admin(user: &AuthUser) -> ApiResult<()> {
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(403, "Only admins manage the AI knowledge"));
    }
    Ok(())
}

fn _dict(d: &AIKnowledgeDoc, content: bool) -> Value {
    let mut out = json!({
        "id": d.id.to_string(), "title": d.title, "description": d.description, "source_type": d.source_type,
        "original_filename": d.original_filename, "original_size": d.original_size,
        "status": d.status, "error": d.error, "is_active": d.is_active, "chunk_count": d.chunk_count,
        "customer_safe": d.customer_safe,
        "use_count": d.use_count, "last_used_at": resp::iso_opt(d.last_used_at.as_ref()),
        "created_at": resp::iso(&d.created_at),
        "updated_at": resp::iso(&d.updated_at),
        "characters": d.content.as_deref().unwrap_or("").chars().count(),
        // The ERP guide: rebuilt from the code, so read-only here (it can be switched off).
        "system": d.system_key.as_deref().is_some_and(|k| !k.is_empty()),
    });
    if content {
        out.as_object_mut().unwrap().insert(
            "content".into(),
            json!(d.content.clone().unwrap_or_default()),
        );
    }
    out
}

// ─── Storage (private bucket) ────────────────────────────────────────────────

fn store_err(e: granary_services::storage::StoreError) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{e}"))
}

async fn _put(object_key: &str, data: &[u8], mime: &str) -> ApiResult<()> {
    let store = granary_services::storage::minio();
    let bucket = granary_core::config::settings()
        .MINIO_BUCKET_DOCUMENTS
        .clone();
    granary_services::storage::ensure_bucket(store.as_ref(), &bucket)
        .await
        .map_err(store_err)?;
    store
        .put_object(&bucket, object_key, data, mime)
        .await
        .map_err(store_err)
}

/// `_remove(object_key)`: a stray file must not block deleting the document.
async fn _remove(object_key: &str) {
    let store = granary_services::storage::minio();
    let bucket = granary_core::config::settings()
        .MINIO_BUCKET_DOCUMENTS
        .clone();
    let _ = store.remove_object(&bucket, object_key).await;
}

/// `_queue(doc_id)`: hand indexing to a worker; false when none answers (the caller then does it inline).
async fn _queue(doc_id: Uuid) -> bool {
    if !granary_services::tasks::ping(std::time::Duration::from_millis(1500)).await {
        return false;
    }
    granary_services::tasks::delay(
        "index_ai_knowledge_doc",
        vec![json!(doc_id.to_string())],
        serde_json::Map::new(),
    )
    .await
    .is_ok()
}

async fn fetch(conn: &mut PgConnection, id: Uuid) -> ApiResult<Option<AIKnowledgeDoc>> {
    Ok(
        sqlx::query_as("SELECT * FROM ai_knowledge_docs WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// `_fresh(db, doc_id)`: the document as stored now.
async fn _fresh(conn: &mut PgConnection, id: Uuid) -> ApiResult<AIKnowledgeDoc> {
    fetch(conn, id)
        .await?
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("document vanished")))
}

/// `_load(db, doc_id)`: 404 for a malformed or unknown id.
async fn _load(conn: &mut PgConnection, doc_id: &str) -> ApiResult<AIKnowledgeDoc> {
    let doc = match granary_compat::text::utils::py_uuid(doc_id) {
        Some(id) => fetch(conn, id).await?,
        None => None,
    };
    doc.ok_or_else(|| ApiError::http(404, "Document not found"))
}

/// `_index_now_or_later(db, doc, user, pdf_bytes)`.
async fn _index_now_or_later(
    db: &Db,
    doc: &mut AIKnowledgeDoc,
    user: &AuthUser,
    pdf_bytes: Option<&[u8]>,
) -> ApiResult<()> {
    doc.status = "processing".into();
    doc.error = None;
    {
        let mut conn = db.conn().await?;
        sqlx::query(
            "UPDATE ai_knowledge_docs SET status = 'processing', error = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(doc.id)
        .execute(&mut *conn)
        .await?;
    }
    db.commit().await?;
    if !_queue(doc.id).await {
        let mut conn = db.conn().await?;
        knowledge::process(&mut conn, doc, pdf_bytes, Some(user.id)).await?;
    }
    Ok(())
}

// ─── Endpoints ───────────────────────────────────────────────────────────────

/// `GET ""`.
pub async fn list_documents(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require_admin(&user)?;
    let mut conn = db.conn().await?;
    let rows: Vec<AIKnowledgeDoc> =
        sqlx::query_as("SELECT * FROM ai_knowledge_docs ORDER BY created_at DESC")
            .fetch_all(&mut *conn)
            .await?;
    Ok(resp::json(json!({
        "items": rows.iter().map(|d| _dict(d, false)).collect::<Vec<_>>(),
    })))
}

/// `os.path.splitext(name)[0]`.
fn splitext_root(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if name[..i].chars().any(|c| c != '.') => &name[..i],
        _ => name,
    }
}

/// `POST ""` (201): upload a Markdown/PDF file, or paste Markdown in `content`.
pub async fn add_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = granary_core::validation::Cx::new();
    let title = form.opt_text("title").unwrap_or_default();
    let description = form.opt_text("description").unwrap_or_default();
    let content = form.opt_text("content").unwrap_or_default();
    let file = form.opt_file(&mut cx, "file");
    form.check(cx)?;
    _require_admin(&user)?;
    use granary_compat::text::pystr::strip;
    let id = Uuid::new_v4();
    let description = Some(strip(&description).to_string()).filter(|d| !d.is_empty());
    let mut pdf_bytes: Option<Vec<u8>> = None;
    let (source_type, doc_content, doc_title, filename, size, object_key);
    match file.filter(|f| !f.filename.is_empty()) {
        Some(f) => {
            let name = f.filename.rsplit('/').next().unwrap_or("").to_string();
            let data = f.data.to_vec();
            let lower = name.to_lowercase();
            if lower.ends_with(".pdf") {
                if data.len() > knowledge::PDF_MAX_BYTES {
                    return Err(ApiError::http(413, "PDF is larger than 25 MB"));
                }
                if !data.starts_with(b"%PDF") {
                    return Err(ApiError::http(400, "That file is not a PDF"));
                }
                source_type = "pdf";
                doc_content = None;
                pdf_bytes = Some(data.clone());
            } else if MARKDOWN_EXT.iter().any(|e| lower.ends_with(e)) {
                if data.len() > MARKDOWN_MAX_BYTES {
                    return Err(ApiError::http(413, "Markdown file is larger than 5 MB"));
                }
                let Ok(text) = String::from_utf8(data.clone()) else {
                    return Err(ApiError::http(400, "The Markdown file must be UTF-8 text"));
                };
                source_type = "markdown";
                doc_content = Some(text);
            } else {
                return Err(ApiError::http(400, "Upload a Markdown (.md) or PDF file"));
            }
            let key = format!("ai-knowledge/{id}/{name}");
            _put(
                &key,
                &data,
                if source_type == "pdf" {
                    "application/pdf"
                } else {
                    "text/markdown"
                },
            )
            .await?;
            let t = strip(&title);
            let t = if t.is_empty() {
                splitext_root(&name)
            } else {
                t
            };
            doc_title = t.chars().take(300).collect::<String>();
            size = Some(data.len() as i32);
            filename = Some(name);
            object_key = Some(key);
        }
        None => {
            if strip(&content).is_empty() {
                return Err(ApiError::http(400, "Upload a file or paste some Markdown"));
            }
            source_type = "markdown";
            doc_content = Some(content.clone());
            if strip(&title).is_empty() {
                return Err(ApiError::http(400, "Give the document a title"));
            }
            doc_title = strip(&title).chars().take(300).collect::<String>();
            filename = None;
            size = None;
            object_key = None;
        }
    }
    {
        let mut conn = db.conn().await?;
        sqlx::query(
            "INSERT INTO ai_knowledge_docs (title, description, source_type, content, original_filename, \
             original_object_key, original_size, status, is_active, customer_safe, chunk_count, use_count, \
             created_by_id, id) VALUES ($1, $2, $3, $4, $5, $6, $7, 'processing', true, false, 0, 0, $8, $9)",
        )
        .bind(&doc_title)
        .bind(&description)
        .bind(source_type)
        .bind(&doc_content)
        .bind(&filename)
        .bind(&object_key)
        .bind(size)
        .bind(user.id)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    let mut doc = {
        let mut conn = db.conn().await?;
        _fresh(&mut conn, id).await?
    };
    _index_now_or_later(&db, &mut doc, &user, pdf_bytes.as_deref()).await?;
    let mut conn = db.conn().await?;
    let doc = _fresh(&mut conn, id).await?;
    Ok(resp::json_status(201, _dict(&doc, false)))
}

async fn get_document_inner(user: &AuthUser, db: &Db, doc_id: &str) -> ApiResult<Response> {
    _require_admin(user)?;
    let mut conn = db.conn().await?;
    let doc = _load(&mut conn, doc_id).await?;
    Ok(resp::json(_dict(&doc, true)))
}

/// `GET /{doc_id}`.
pub async fn get_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    get_document_inner(&user, &db, &doc_id).await
}

/// `GET /search`: Starlette matches `/{doc_id}` first.
pub async fn get_search_as_document(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    get_document_inner(&user, &db, "search").await
}

granary_core::py_model! {
    /// `class DocUpdate(BaseModel)`.
    pub struct DocUpdate {
        pub title: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(300))),
        pub description: Option<String> = None,
        pub is_active: Option<bool> = None,
        /// Nary may use it when talking to a customer
        pub customer_safe: Option<bool> = None,
        pub content: Option<String> = None,
    }
}

async fn update_document_inner(
    user: &AuthUser,
    db: &Db,
    doc_id: &str,
    body: DocUpdate,
) -> ApiResult<Response> {
    use granary_compat::text::pystr::strip;
    _require_admin(user)?;
    let mut doc = {
        let mut conn = db.conn().await?;
        _load(&mut conn, doc_id).await?
    };
    let orig = doc.clone();
    let is_system = doc.system_key.as_deref().is_some_and(|k| !k.is_empty());
    if is_system && (body.title.is_some() || body.description.is_some() || body.content.is_some()) {
        return Err(ApiError::http(
            400,
            "This is the ERP guide, built from the code: it can only be switched on or off. \
             Add your own document for anything else.",
        ));
    }
    let mut reindex = false;
    if let Some(t) = body.title.as_deref().map(strip).filter(|t| !t.is_empty()) {
        reindex |= t != doc.title;
        doc.title = t.to_string();
    }
    if let Some(d) = body.description.as_deref() {
        let d = Some(strip(d).to_string()).filter(|d| !d.is_empty());
        reindex |= d != doc.description;
        doc.description = d;
    }
    if let Some(a) = body.is_active {
        doc.is_active = a;
    }
    if let Some(c) = body.customer_safe {
        doc.customer_safe = c;
    }
    if let Some(c) = body.content.as_deref() {
        if c != doc.content.as_deref().unwrap_or("") {
            if strip(c).is_empty() {
                return Err(ApiError::http(400, "The content can't be empty"));
            }
            doc.content = Some(c.to_string());
            reindex = true;
        }
    }
    // The unit of work writes only when an attribute really changed (then onupdate bumps updated_at).
    let changed = (
        &doc.title,
        &doc.description,
        doc.is_active,
        doc.customer_safe,
        &doc.content,
    ) != (
        &orig.title,
        &orig.description,
        orig.is_active,
        orig.customer_safe,
        &orig.content,
    );
    if changed {
        let mut conn = db.conn().await?;
        sqlx::query(
            "UPDATE ai_knowledge_docs SET title = $2, description = $3, is_active = $4, customer_safe = $5, \
             content = $6, updated_at = now() WHERE id = $1",
        )
        .bind(doc.id)
        .bind(&doc.title)
        .bind(&doc.description)
        .bind(doc.is_active)
        .bind(doc.customer_safe)
        .bind(&doc.content)
        .execute(&mut *conn)
        .await?;
    }
    db.commit().await?;
    if reindex {
        _index_now_or_later(db, &mut doc, user, None).await?;
    }
    let mut conn = db.conn().await?;
    let doc = _fresh(&mut conn, doc.id).await?;
    Ok(resp::json(_dict(&doc, true)))
}

/// `PATCH /{doc_id}`.
pub async fn update_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<DocUpdate>,
) -> ApiResult<Response> {
    update_document_inner(&user, &db, &doc_id, body).await
}

/// `PATCH /search`: Starlette matches `/{doc_id}` first.
pub async fn update_search_as_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<DocUpdate>,
) -> ApiResult<Response> {
    update_document_inner(&user, &db, "search", body).await
}

/// `POST /{doc_id}/reindex`.
pub async fn reindex_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    _require_admin(&user)?;
    let mut doc = {
        let mut conn = db.conn().await?;
        _load(&mut conn, &doc_id).await?
    };
    _index_now_or_later(&db, &mut doc, &user, None).await?;
    let mut conn = db.conn().await?;
    let doc = _fresh(&mut conn, doc.id).await?;
    Ok(resp::json(_dict(&doc, false)))
}

async fn delete_document_inner(user: &AuthUser, db: &Db, doc_id: &str) -> ApiResult<Response> {
    _require_admin(user)?;
    let key = {
        let mut conn = db.conn().await?;
        let doc = _load(&mut conn, doc_id).await?;
        if doc.system_key.as_deref().is_some_and(|k| !k.is_empty()) {
            return Err(ApiError::http(
                400,
                "This is the ERP guide, built from the code: switch it off instead (or turn off \
                 ai.erp_guide.enabled).",
            ));
        }
        sqlx::query("DELETE FROM ai_knowledge_chunks WHERE ai_knowledge_chunks.doc_id = $1")
            .bind(doc.id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM ai_knowledge_docs WHERE ai_knowledge_docs.id = $1")
            .bind(doc.id)
            .execute(&mut *conn)
            .await?;
        doc.original_object_key
    };
    db.commit().await?;
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        _remove(&k).await;
    }
    Ok(resp::no_content())
}

/// `DELETE /{doc_id}` (204).
pub async fn delete_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    delete_document_inner(&user, &db, &doc_id).await
}

/// `DELETE /search`: Starlette matches `/{doc_id}` first.
pub async fn delete_search_as_document(
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    delete_document_inner(&user, &db, "search").await
}

granary_core::py_model! {
    /// `class SearchBody(BaseModel)`.
    pub struct SearchBody {
        pub query: String => v::str_c(v::StrC::new().min_length(1).max_length(2000)),
    }
}

/// `POST /search`: what the AI would retrieve for this question.
pub async fn test_search(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SearchBody>,
) -> ApiResult<Response> {
    _require_admin(&user)?;
    let mut conn = db.conn().await?;
    let hits = match knowledge::search(&mut conn, &body.query, 6, None, false).await {
        Ok(h) => h,
        Err(exc) => {
            return Err(ApiError::http(
                400,
                format!("Search failed (is an embedding provider set up?): {exc}"),
            ))
        }
    };
    drop(conn);
    db.commit().await?;
    let results: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({"doc_id": h.doc_id, "title": h.title,
                   "content": h.content.chars().take(600).collect::<String>(),
                   "distance": resp::py_float(h.distance),
                   "used": h.distance <= knowledge::MAX_DISTANCE})
        })
        .collect();
    Ok(resp::json(json!({"results": results})))
}
