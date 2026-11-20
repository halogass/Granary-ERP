//! Batch 4 catalog product files become library documents (run once at startup, guarded by the setting
//! documents.product_files_migrated).
//!
//! Every `attachments` row with entity_type 'catalog_product' that is a PDF or an image becomes a document:
//! category products, audiences {customer}, owner = the uploader (else the Owner), title = the file name without
//! its extension, linked to the product; one version v1 "Moved from the catalog product files", live, same
//! object key (the bytes are copied into the private documents bucket under that key), text extracted (the
//! text batch 4 already read for the same content when there is one; otherwise locally, no vision at startup).
//! The attachment rows stay (nothing breaks before the frontend switches); the attachments list stops showing
//! the migrated ones. Other file types (DOCX, XLSX) stay attachments: a version must be a PDF or an image.
//! Idempotent even without the flag: a product file already in the library (same key, same product) is skipped.

use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_core::config::settings;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::get_optional;

use super::versions::sendable_kind;
use crate::nary::file_text;
use crate::nary::files as nary_files;
use crate::storage::{attachment_bucket, ensure_bucket, minio};

pub const FLAG: &str = "documents.product_files_migrated";
pub const NOTE: &str = "Moved from the catalog product files";

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub migrated: usize,
    pub skipped_existing: usize,
    pub skipped_type: usize,
    pub failed: usize,
    /// the flag was already set: nothing was looked at
    pub already_done: bool,
}

/// At startup: run once, log the outcome, never fail the server.
pub async fn run_at_startup(pool: PgPool) {
    let mut conn = match pool.acquire().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "document library: product files migration skipped (no database)");
            return;
        }
    };
    match run(&mut conn).await {
        Ok(r) if r.already_done => {}
        Ok(r) => tracing::info!(
            ?r,
            "document library: catalog product files moved into the library"
        ),
        Err(e) => {
            tracing::warn!(error = ?e, "document library: product files migration failed (retried at the next start)")
        }
    }
}

/// Move the product files (see the module doc). Sets the flag when every file was handled.
pub async fn run(db: &mut PgConnection) -> ApiResult<Report> {
    let mut report = Report::default();
    if get_optional(db, FLAG).await?.as_deref().map(str::trim) == Some("true") {
        report.already_done = true;
        return Ok(report);
    }
    let rows: Vec<(Uuid, String, Option<Uuid>, String, String, String)> = sqlx::query_as(
        "SELECT a.id, a.entity_id, a.uploaded_by_id, a.file_name, a.mime_type, a.object_key \
           FROM attachments a JOIN product_catalog p ON p.id::text = a.entity_id \
          WHERE a.entity_type = 'catalog_product' ORDER BY a.created_at, a.id",
    )
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        set_flag(db).await?;
        return Ok(report);
    }
    let category: Uuid = sqlx::query_scalar("SELECT id FROM doc_categories WHERE key = 'products'")
        .fetch_one(&mut *db)
        .await?;
    let owner: Option<Uuid> = crate::approvals::routing::owner_user_ids(db)
        .await?
        .first()
        .and_then(|s| Uuid::parse_str(s).ok());
    for (_aid, product, uploader, name, mime, key) in rows {
        let Ok(product_id) = Uuid::parse_str(&product) else {
            report.failed += 1;
            continue;
        };
        let exists: Option<Uuid> = sqlx::query_scalar(
            "SELECT v.id FROM library_document_versions v \
               JOIN library_document_products lp ON lp.document_id = v.document_id \
              WHERE v.file_object_key = $1 AND lp.product_id = $2 LIMIT 1",
        )
        .bind(&key)
        .bind(product_id)
        .fetch_optional(&mut *db)
        .await?;
        if exists.is_some() {
            report.skipped_existing += 1;
            continue;
        }
        let mime = mime.to_lowercase();
        let Some(kind) = sendable_kind(&name, &mime) else {
            report.skipped_type += 1;
            continue;
        };
        match one(
            db, category, owner, product_id, uploader, &name, &mime, &key, kind,
        )
        .await
        {
            Ok(()) => report.migrated += 1,
            Err(e) => {
                tracing::warn!(error = ?e, key = %key, "document library: a product file could not be moved");
                report.failed += 1;
            }
        }
    }
    if report.failed == 0 {
        set_flag(db).await?;
    }
    Ok(report)
}

async fn set_flag(db: &mut PgConnection) -> ApiResult<()> {
    sqlx::query("UPDATE system_settings SET value = 'true', updated_at = now() WHERE key = $1")
        .bind(FLAG)
        .execute(&mut *db)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn one(
    db: &mut PgConnection,
    category: Uuid,
    owner: Option<Uuid>,
    product_id: Uuid,
    uploader: Option<Uuid>,
    name: &str,
    mime: &str,
    key: &str,
    kind: &str,
) -> ApiResult<()> {
    let store = minio();
    let data = store
        .get_object(&attachment_bucket(key), key)
        .await
        .map_err(ApiError::internal)?;
    let docs_bucket = settings().MINIO_BUCKET_DOCUMENTS.clone();
    if attachment_bucket(key) != docs_bucket {
        ensure_bucket(&*store, &docs_bucket)
            .await
            .map_err(ApiError::internal)?;
        store
            .put_object(&docs_bucket, key, &data, mime)
            .await
            .map_err(ApiError::internal)?;
    }
    let sha = hex::encode(Sha256::digest(&data));
    // the text batch 4 already read for this content, else locally (no vision at startup)
    let cached: Option<(Option<serde_json::Value>, Option<i32>)> = sqlx::query_as(
        "SELECT units, page_count FROM nary_files WHERE sha256 = $1 AND status = 'done'",
    )
    .bind(&sha)
    .fetch_optional(&mut *db)
    .await?;
    let (units, pages) = match cached {
        Some((Some(u), p)) => (Some(u), p),
        _ => {
            let mut caps = nary_files::caps(db).await?;
            caps.vision_cap = 0;
            match nary_files::units_with(db, &data, name, mime, kind, None, caps).await {
                Ok((u, p, _)) => (Some(file_text::units_json(&u)), p),
                Err(_) => (None, if kind == "image" { Some(1) } else { None }),
            }
        }
    };
    let owner: Option<Uuid> = match uploader {
        Some(u) => sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
            .bind(u)
            .fetch_optional(&mut *db)
            .await?
            .or(owner),
        None => owner,
    };
    let title: String = {
        let stem = match name.rsplit_once('.') {
            Some((s, _)) if !s.trim().is_empty() => s.trim(),
            _ => name.trim(),
        };
        stem.chars().take(300).collect()
    };
    let doc_id = Uuid::new_v4();
    let vid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO library_documents (id, title, category_id, audiences, owner_id, language, kind, created_by) \
         VALUES ($1, $2, $3, '{customer}', $4, 'en', 'uploaded', $4)",
    )
    .bind(doc_id)
    .bind(&title)
    .bind(category)
    .bind(owner)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "INSERT INTO library_document_products (id, document_id, product_id) VALUES ($1, $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(doc_id)
    .bind(product_id)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "INSERT INTO library_document_versions (id, document_id, label, change_note, status, file_object_key, \
         file_name, file_mime, file_size, file_sha256, page_count, extracted_units, extracted_at, uploaded_by, \
         went_live_at) VALUES ($1, $2, 'v1', $3, 'live', $4, $5, $6, $7, $8, $9, $10, \
         CASE WHEN $10::jsonb IS NULL THEN NULL ELSE now() END, $11, now())",
    )
    .bind(vid)
    .bind(doc_id)
    .bind(NOTE)
    .bind(key)
    .bind(name.chars().take(255).collect::<String>())
    .bind(mime.chars().take(120).collect::<String>())
    .bind(data.len() as i64)
    .bind(&sha)
    .bind(pages)
    .bind(units)
    .bind(owner)
    .execute(&mut *db)
    .await?;
    sqlx::query("UPDATE library_documents SET current_version_id = $2 WHERE id = $1")
        .bind(doc_id)
        .bind(vid)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// The object keys of catalog product files now in the library (the attachments list hides them).
pub async fn migrated_keys(db: &mut PgConnection, product_id: &str) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT v.file_object_key FROM library_document_versions v \
           JOIN library_document_products lp ON lp.document_id = v.document_id \
          WHERE lp.product_id::text = $1 AND v.change_note = $2",
    )
    .bind(product_id)
    .bind(NOTE)
    .fetch_all(&mut *db)
    .await?)
}
