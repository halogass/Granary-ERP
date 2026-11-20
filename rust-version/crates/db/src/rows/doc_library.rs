//! doclib A1: rows of the document library (migration 0117). Hand-written: the tables are Rust only (the
//! Python backend is frozen), so compat-tests/schema/gen_rows.py does not produce them.

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

/// Row of `doc_categories`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocCategory {
    pub id: Uuid,
    /// company | products | pricing | technical | templates
    pub key: String,
    pub name: String,
    pub sort_order: i32,
    /// Role slugs that see the category; empty = every internal staff member.
    pub visible_roles: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DocCategory {
    pub const TABLE: &'static str = "doc_categories";
    pub const COLUMNS: &'static [&'static str] = &[
        "id",
        "key",
        "name",
        "sort_order",
        "visible_roles",
        "created_at",
        "updated_at",
    ];
}

/// Row of `library_documents`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibraryDocument {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub category_id: Uuid,
    /// Subset of internal | customer | partner (at least one).
    pub audiences: Vec<String>,
    pub owner_id: Option<Uuid>,
    /// ISO 639-1
    pub language: String,
    pub language_group_id: Option<Uuid>,
    pub review_by: Option<NaiveDate>,
    pub last_reminded_at: Option<DateTime<Utc>>,
    pub current_version_id: Option<Uuid>,
    /// uploaded | price_list
    pub kind: String,
    pub is_deleted: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LibraryDocument {
    pub const TABLE: &'static str = "library_documents";
    pub const COLUMNS: &'static [&'static str] = &[
        "id",
        "title",
        "description",
        "category_id",
        "audiences",
        "owner_id",
        "language",
        "language_group_id",
        "review_by",
        "last_reminded_at",
        "current_version_id",
        "kind",
        "is_deleted",
        "created_by",
        "created_at",
        "updated_at",
    ];

    pub fn has_audience(&self, a: &str) -> bool {
        self.audiences.iter().any(|x| x == a)
    }

    /// Any customer or partner audience: versions need executive approval.
    pub fn is_external(&self) -> bool {
        self.has_audience("customer") || self.has_audience("partner")
    }
}

/// Row of `library_document_products` (product_id or family, never both).
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibraryDocumentProduct {
    pub id: Uuid,
    pub document_id: Uuid,
    pub product_id: Option<Uuid>,
    pub family: Option<String>,
}

impl LibraryDocumentProduct {
    pub const TABLE: &'static str = "library_document_products";
    pub const COLUMNS: &'static [&'static str] = &["id", "document_id", "product_id", "family"];
}

/// Row of `library_document_versions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibraryDocumentVersion {
    pub id: Uuid,
    pub document_id: Uuid,
    pub label: String,
    pub change_note: String,
    /// draft | pending_approval | live | superseded | rejected
    pub status: String,
    pub file_object_key: String,
    pub file_name: String,
    pub file_mime: String,
    pub file_size: i64,
    pub file_sha256: String,
    pub page_count: Option<i32>,
    pub source_object_key: Option<String>,
    pub source_file_name: Option<String>,
    pub source_mime: Option<String>,
    pub source_size: Option<i64>,
    /// batch 4 `file_text::Unit` list ([{label, mode, text}])
    pub extracted_units: Option<serde_json::Value>,
    pub extracted_at: Option<DateTime<Utc>>,
    pub uploaded_by: Option<Uuid>,
    pub approved_by: Option<Uuid>,
    pub approved_at: Option<DateTime<Utc>>,
    pub rejected_reason: Option<String>,
    pub went_live_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// 5 Nov 2026 (migration 0162): a video's length in seconds
    pub duration_seconds: Option<f64>,
    /// 5 Nov 2026 (migration 0162): documents (the private documents bucket) | assets (a CRM attachment's
    /// own object, stored once)
    pub file_bucket: String,
}

impl LibraryDocumentVersion {
    pub const TABLE: &'static str = "library_document_versions";
    pub const COLUMNS: &'static [&'static str] = &[
        "id",
        "document_id",
        "label",
        "change_note",
        "status",
        "file_object_key",
        "file_name",
        "file_mime",
        "file_size",
        "file_sha256",
        "page_count",
        "source_object_key",
        "source_file_name",
        "source_mime",
        "source_size",
        "extracted_units",
        "extracted_at",
        "uploaded_by",
        "approved_by",
        "approved_at",
        "rejected_reason",
        "went_live_at",
        "created_at",
        "duration_seconds",
        "file_bucket",
    ];
}

/// Row of `library_downloads` (the fingerprint log).
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibraryDownload {
    pub id: Uuid,
    /// `ID-` + 10 Crockford base32 characters
    pub fingerprint: String,
    pub version_id: Uuid,
    pub document_id: Uuid,
    /// staff_download | customer_send | partner_download | nary_send | source_download
    pub kind: String,
    pub user_id: Option<Uuid>,
    pub partner_account_id: Option<Uuid>,
    pub recipient_name: Option<String>,
    pub recipient_company: Option<String>,
    /// whatsapp | email | portal | web
    pub channel: Option<String>,
    pub file_sha256: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl LibraryDownload {
    pub const TABLE: &'static str = "library_downloads";
    pub const COLUMNS: &'static [&'static str] = &[
        "id",
        "fingerprint",
        "version_id",
        "document_id",
        "kind",
        "user_id",
        "partner_account_id",
        "recipient_name",
        "recipient_company",
        "channel",
        "file_sha256",
        "ip",
        "user_agent",
        "created_at",
    ];
}

/// Row of `library_article_links`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibraryArticleLink {
    pub article_id: Uuid,
    pub document_id: Uuid,
    pub created_at: DateTime<Utc>,
}

impl LibraryArticleLink {
    pub const TABLE: &'static str = "library_article_links";
    pub const COLUMNS: &'static [&'static str] = &["article_id", "document_id", "created_at"];
}
