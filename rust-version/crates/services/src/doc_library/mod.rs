//! The document library (21 Oct 2026, Rust only): the company's own documents in the Knowledge Base (company
//! profile, brochures, datasheets, system requirements), used by staff and by Nary.
//!
//! ```text
//!     documents      library_documents: title, category (doc_categories, visible to some roles or all),
//!                    audiences internal|customer|partner, owner, language (+ language group), review date,
//!                    products or families, current live version
//!     versions       versions.rs: label (suggest_label), change note, status draft|pending_approval|live|
//!                    superseded|rejected. Internal-only documents go live at once; any customer or partner
//!                    audience goes through the approval engine (adapter library_document_version,
//!                    approvers = documents:manage_all). The sent file is a PDF or an image; a DOCX/XLSX/PPTX
//!                    source may ride along (never sent). Text extracted on upload (batch 4 file_text).
//!     access         access.rs: who may view (documents:view + category roles), edit (owner with
//!                    documents:create, or documents:manage_all), see history / restore / trace (doc admins)
//!     copies         A2: stamp.rs (watermark footer + hidden fingerprint), fingerprint.rs (library_downloads,
//!                    trace), deliver.rs (customer copies), price_list.rs (the internal price list PDF)
//!     notify         notify.rs: new live version -> staff who can view; daily review reminders to owners
//!     product files  migrate_product_files.rs: batch 4 catalog product files become library documents, once
//! ```

pub mod access;
pub mod deliver; // A2
pub mod fingerprint; // A2
pub mod migrate_product_files;
pub mod notify;
pub mod price_list; // A2
pub mod stamp; // A2
pub mod versions;
pub mod view;

use serde::Deserialize;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;

pub use granary_db::rows::doc_library::{
    DocCategory as Category, LibraryDocument as Document, LibraryDocumentVersion as Version,
};

/// Where messages and errors of this module say they come from (settings errors).
pub const WHERE: &str = "Company & branding → Knowledge & documents";

/// The three audiences a document may have.
pub const AUDIENCES: [&str; 3] = ["internal", "customer", "partner"];

/// Version statuses.
pub const STATUS_DRAFT: &str = "draft";
pub const STATUS_PENDING: &str = "pending_approval";
pub const STATUS_LIVE: &str = "live";
pub const STATUS_SUPERSEDED: &str = "superseded";
pub const STATUS_REJECTED: &str = "rejected";

/// The private documents bucket prefix of library files (`doclib/<document id>/<sha256>-<name>`).
pub const OBJECT_PREFIX: &str = "doclib/";

pub(crate) const DOC_COLS: &str = "d.id, d.title, d.description, d.category_id, d.audiences, d.owner_id, d.language, \
    d.language_group_id, d.review_by, d.last_reminded_at, d.current_version_id, d.kind, d.is_deleted, d.created_by, \
    d.created_at, d.updated_at";
pub(crate) const VERSION_COLS: &str = "id, document_id, label, change_note, status, file_object_key, file_name, \
    file_mime, file_size, file_sha256, page_count, source_object_key, source_file_name, source_mime, source_size, \
    extracted_units, extracted_at, uploaded_by, approved_by, approved_at, rejected_reason, went_live_at, created_at, \
    duration_seconds, file_bucket";

/// A document (not deleted) by id.
pub async fn load_document(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Document>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {DOC_COLS} FROM library_documents d WHERE d.id = $1 AND d.is_deleted = false"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

/// A version by id.
pub async fn load_version(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Version>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {VERSION_COLS} FROM library_document_versions WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

/// Every version of a document, newest first.
pub async fn versions_of(db: &mut PgConnection, document_id: Uuid) -> ApiResult<Vec<Version>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {VERSION_COLS} FROM library_document_versions WHERE document_id = $1 \
         ORDER BY created_at DESC, id DESC"
    ))
    .bind(document_id)
    .fetch_all(&mut *db)
    .await?)
}

/// The live version of a document (None while nothing went live yet, or for a deleted document).
pub async fn current_live(db: &mut PgConnection, document_id: Uuid) -> ApiResult<Option<Version>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {} FROM library_document_versions v \
           JOIN library_documents d ON d.id = v.document_id AND d.current_version_id = v.id \
          WHERE v.document_id = $1 AND v.status = 'live' AND d.is_deleted = false",
        VERSION_COLS
            .split(", ")
            .map(|c| format!("v.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(document_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// The version waiting for approval, if any.
pub async fn pending_version(
    db: &mut PgConnection,
    document_id: Uuid,
) -> ApiResult<Option<Version>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {VERSION_COLS} FROM library_document_versions WHERE document_id = $1 \
         AND status = 'pending_approval' ORDER BY created_at DESC LIMIT 1"
    ))
    .bind(document_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// The label after `prev`: the last number in it + 1, the rest kept (v33 -> v34, v2.1 -> v2.2, 3 -> 4,
/// "Rev B2" -> "Rev B3"); a label without a number gets " 2" appended; no previous version -> v1.
pub fn suggest_label(prev: Option<&str>) -> String {
    let Some(prev) = prev.map(str::trim).filter(|p| !p.is_empty()) else {
        return "v1".to_string();
    };
    let chars: Vec<char> = prev.chars().collect();
    let Some(end) = chars.iter().rposition(|c| c.is_ascii_digit()) else {
        return format!("{prev} 2");
    };
    let mut start = end;
    while start > 0 && chars[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let digits: String = chars[start..=end].iter().collect();
    let next = match digits.parse::<u128>() {
        Ok(n) => {
            let s = (n + 1).to_string();
            // keep leading zeros (v09 -> v10, v007 -> v008)
            if s.len() < digits.len() {
                format!("{:0>width$}", s, width = digits.len())
            } else {
                s
            }
        }
        Err(_) => return format!("{prev} 2"),
    };
    let head: String = chars[..start].iter().collect();
    let tail: String = chars[end + 1..].iter().collect();
    format!("{head}{next}{tail}")
}

/// The label suggested for the next version of a document (from its newest version).
pub async fn next_label(db: &mut PgConnection, document_id: Uuid) -> ApiResult<String> {
    let labels: Vec<String> = sqlx::query_scalar(
        "SELECT label FROM library_document_versions WHERE document_id = $1 ORDER BY created_at DESC, id DESC",
    )
    .bind(document_id)
    .fetch_all(&mut *db)
    .await?;
    let mut label = suggest_label(labels.first().map(String::as_str));
    // never suggest a label already taken
    while labels.iter().any(|l| l == &label) {
        label = suggest_label(Some(&label));
    }
    Ok(label)
}

/// Who is searching.
#[derive(Debug, Clone)]
pub enum Scope<'a> {
    /// A staff member: documents:view plus the category's roles (directors and doc admins see everything).
    Staff(&'a AuthUser),
    /// No user: only documents with one of these audiences that have a live version (Nary drafting to a
    /// customer, the partner portal). Category visibility is ignored.
    Audiences(Vec<String>),
}

/// Search filters (the list endpoint's query, the Nary tools).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filters {
    /// all | mine | customer | partner | internal | needs_review | pending
    pub view: Option<String>,
    /// a category key
    pub category: Option<String>,
    /// internal | customer | partner
    pub audience: Option<String>,
    pub language: Option<String>,
    /// documents linked to this product, or to its family
    pub product_id: Option<Uuid>,
    pub family: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// only documents with a live version
    #[serde(default)]
    pub only_live: bool,
    /// include price_list documents (never for Nary or customers)
    #[serde(default)]
    pub include_price_list: bool,
}

/// One page of documents and how many match in all.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub items: Vec<Document>,
    pub total: i64,
}

/// Documents the scope may see that match the query (title, description, the live text) and filters,
/// newest first. Limit default 50, at most 200.
pub async fn search(
    db: &mut PgConnection,
    scope: &Scope<'_>,
    query: Option<&str>,
    filters: &Filters,
) -> ApiResult<SearchResult> {
    let mut wh: Vec<String> = vec!["d.is_deleted = false".into()];
    let mut args: Vec<Arg> = Vec::new();
    let push = |args: &mut Vec<Arg>, a: Arg| -> usize {
        args.push(a);
        args.len()
    };
    let me: Option<Uuid> = match scope {
        Scope::Staff(user) => {
            if !access::sees_all_categories(user) {
                let roles = access::role_slugs(db, user.id).await?;
                let n = push(&mut args, Arg::Texts(roles));
                wh.push(format!(
                    "EXISTS (SELECT 1 FROM doc_categories c WHERE c.id = d.category_id \
                     AND (cardinality(c.visible_roles) = 0 OR c.visible_roles && ${n}::text[]))"
                ));
            }
            Some(user.id)
        }
        Scope::Audiences(aud) => {
            let n = push(&mut args, Arg::Texts(aud.clone()));
            wh.push(format!("d.audiences && ${n}::text[]"));
            wh.push("d.current_version_id IS NOT NULL".into());
            wh.push("d.kind <> 'price_list'".into());
            None
        }
    };
    if !filters.include_price_list && matches!(scope, Scope::Staff(_)) {
        // the price list only where asked for (the library screens); Nary never gets it
        wh.push("d.kind <> 'price_list'".into());
    }
    if filters.only_live {
        wh.push("d.current_version_id IS NOT NULL".into());
    }
    match filters.view.as_deref().unwrap_or("all") {
        "mine" => {
            if let Some(me) = me {
                let n = push(&mut args, Arg::Uuid(me));
                wh.push(format!("(d.owner_id = ${n} OR d.created_by = ${n})"));
            }
        }
        v @ ("customer" | "partner" | "internal") => {
            let n = push(&mut args, Arg::Text(v.to_string()));
            wh.push(format!("${n} = ANY(d.audiences)"));
        }
        "needs_review" => wh.push("d.review_by IS NOT NULL AND d.review_by <= CURRENT_DATE".into()),
        "pending" => wh.push(
            "EXISTS (SELECT 1 FROM library_document_versions pv WHERE pv.document_id = d.id \
             AND pv.status = 'pending_approval')"
                .into(),
        ),
        _ => {}
    }
    if let Some(c) = filters.category.as_deref().filter(|s| !s.is_empty()) {
        let n = push(&mut args, Arg::Text(c.to_string()));
        wh.push(format!(
            "d.category_id IN (SELECT id FROM doc_categories WHERE key = ${n})"
        ));
    }
    if let Some(a) = filters.audience.as_deref().filter(|s| !s.is_empty()) {
        let n = push(&mut args, Arg::Text(a.to_string()));
        wh.push(format!("${n} = ANY(d.audiences)"));
    }
    if let Some(l) = filters.language.as_deref().filter(|s| !s.is_empty()) {
        let n = push(&mut args, Arg::Text(l.to_lowercase()));
        wh.push(format!("d.language = ${n}"));
    }
    if let Some(p) = filters.product_id {
        let n = push(&mut args, Arg::Uuid(p));
        wh.push(format!(
            "EXISTS (SELECT 1 FROM library_document_products lp WHERE lp.document_id = d.id \
             AND (lp.product_id = ${n} OR lp.family = (SELECT family FROM product_catalog WHERE id = ${n})))"
        ));
    }
    if let Some(f) = filters.family.as_deref().filter(|s| !s.is_empty()) {
        let n = push(&mut args, Arg::Text(f.to_string()));
        wh.push(format!(
            "EXISTS (SELECT 1 FROM library_document_products lp LEFT JOIN product_catalog pc ON pc.id = lp.product_id \
             WHERE lp.document_id = d.id AND (lp.family = ${n} OR pc.family = ${n}))"
        ));
    }
    if let Some(q) = query.map(str::trim).filter(|s| !s.is_empty()) {
        let like = format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let n = push(&mut args, Arg::Text(like));
        wh.push(format!(
            "(d.title ILIKE ${n} OR d.description ILIKE ${n} OR EXISTS (SELECT 1 FROM library_document_versions sv \
             WHERE sv.id = d.current_version_id AND (sv.file_name ILIKE ${n} OR sv.label ILIKE ${n} \
             OR sv.extracted_units::text ILIKE ${n})) \
             OR EXISTS (SELECT 1 FROM library_document_products lp LEFT JOIN product_catalog pc ON pc.id = lp.product_id \
             WHERE lp.document_id = d.id AND (lp.family ILIKE ${n} OR pc.name ILIKE ${n} OR pc.sku ILIKE ${n})))"
        ));
    }
    let where_sql = wh.join(" AND ");
    let limit = filters.limit.unwrap_or(50).clamp(1, 200);
    let offset = filters.offset.unwrap_or(0).max(0);

    let count_sql = format!("SELECT count(*) FROM library_documents d WHERE {where_sql}");
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    for a in &args {
        cq = a.bind_scalar(cq);
    }
    let total = cq.fetch_one(&mut *db).await?;

    let list_sql = format!(
        "SELECT {DOC_COLS} FROM library_documents d WHERE {where_sql} \
         ORDER BY d.updated_at DESC, d.id LIMIT {limit} OFFSET {offset}"
    );
    let mut lq = sqlx::query_as::<_, Document>(&list_sql);
    for a in &args {
        lq = a.bind_as(lq);
    }
    let items = lq.fetch_all(&mut *db).await?;
    Ok(SearchResult { items, total })
}

/// A bind value of [`search`].
#[derive(Debug, Clone)]
enum Arg {
    Text(String),
    Texts(Vec<String>),
    Uuid(Uuid),
}

type PgArgs = sqlx::postgres::PgArguments;

impl Arg {
    fn bind_scalar<'q>(
        &self,
        q: sqlx::query::QueryScalar<'q, sqlx::Postgres, i64, PgArgs>,
    ) -> sqlx::query::QueryScalar<'q, sqlx::Postgres, i64, PgArgs> {
        match self.clone() {
            Arg::Text(s) => q.bind(s),
            Arg::Texts(v) => q.bind(v),
            Arg::Uuid(u) => q.bind(u),
        }
    }

    fn bind_as<'q>(
        &self,
        q: sqlx::query::QueryAs<'q, sqlx::Postgres, Document, PgArgs>,
    ) -> sqlx::query::QueryAs<'q, sqlx::Postgres, Document, PgArgs> {
        match self.clone() {
            Arg::Text(s) => q.bind(s),
            Arg::Texts(v) => q.bind(v),
            Arg::Uuid(u) => q.bind(u),
        }
    }
}

/// The category of a document.
pub async fn category(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Category>> {
    Ok(sqlx::query_as(
        "SELECT id, key, name, sort_order, visible_roles, created_at, updated_at FROM doc_categories WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

/// Every category in order.
pub async fn categories(db: &mut PgConnection) -> ApiResult<Vec<Category>> {
    Ok(sqlx::query_as(
        "SELECT id, key, name, sort_order, visible_roles, created_at, updated_at FROM doc_categories \
         ORDER BY sort_order, name",
    )
    .fetch_all(&mut *db)
    .await?)
}

/// Read a stored library file from the private documents bucket.
pub async fn read_object(key: &str) -> ApiResult<Vec<u8>> {
    crate::storage::minio()
        .get_object(
            &granary_core::config::settings().MINIO_BUCKET_DOCUMENTS,
            key,
        )
        .await
        .map_err(granary_core::errors::ApiError::internal)
}

/// 5 Nov 2026 (store once): `file_bucket` of a version whose file is in the private documents bucket.
pub const BUCKET_DOCUMENTS: &str = "documents";
/// 5 Nov 2026 (store once): `file_bucket` of a version that references a CRM attachment's object in the
/// assets bucket (no copy).
pub const BUCKET_ASSETS: &str = "assets";

/// The storage bucket a version's `file_bucket` names.
pub fn bucket_name(file_bucket: &str) -> String {
    let s = granary_core::config::settings();
    if file_bucket == BUCKET_ASSETS {
        s.MINIO_BUCKET_ASSETS.clone()
    } else {
        s.MINIO_BUCKET_DOCUMENTS.clone()
    }
}

/// The `file_bucket` value of a storage bucket name (None for a bucket the library does not reference).
pub fn bucket_value(bucket: &str) -> Option<&'static str> {
    let s = granary_core::config::settings();
    if bucket == s.MINIO_BUCKET_DOCUMENTS {
        Some(BUCKET_DOCUMENTS)
    } else if bucket == s.MINIO_BUCKET_ASSETS {
        Some(BUCKET_ASSETS)
    } else {
        None
    }
}

/// The sent file of a version, wherever it is stored (its own copy, or the CRM attachment it references).
pub async fn read_file(version: &Version) -> ApiResult<Vec<u8>> {
    crate::storage::minio()
        .get_object(&bucket_name(&version.file_bucket), &version.file_object_key)
        .await
        .map_err(granary_core::errors::ApiError::internal)
}

/// 5 Nov 2026: a video version (MP4, MOV, WebM): logged on every copy, never watermarked.
pub fn is_video(version: &Version) -> bool {
    crate::nary::uploads::is_video(&version.file_mime)
}

/// 5 Nov 2026 (store once): does any library version use this stored object? (Its attachment's delete then
/// keeps the object.)
pub async fn references_object(db: &mut PgConnection, key: &str) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM library_document_versions WHERE file_object_key = $1)",
    )
    .bind(key)
    .fetch_one(&mut *db)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::suggest_label;

    #[test]
    fn labels() {
        for (prev, next) in [
            (None, "v1"),
            (Some("v33"), "v34"),
            (Some("v2.1"), "v2.2"),
            (Some("3"), "4"),
            (Some("Rev B2"), "Rev B3"),
            (Some("final"), "final 2"),
            (Some("v9"), "v10"),
            (Some("v09"), "v10"),
            (Some("2024-03 r1 draft"), "2024-03 r2 draft"),
        ] {
            assert_eq!(suggest_label(prev), next, "{prev:?}");
        }
    }
}
