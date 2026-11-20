// Row structs of the knowledge tables: one struct per table, its columns in order.

/// Row of `kb_article_revisions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KBArticleRevision {
    /// UUID NOT NULL
    pub article_id: uuid::Uuid,
    /// VARCHAR(20) NOT NULL
    pub version: String,
    /// TEXT
    pub content_snapshot: Option<String>,
    /// TEXT
    pub change_summary: Option<String>,
    /// UUID
    pub changed_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl KBArticleRevision {
    pub const TABLE: &'static str = "kb_article_revisions";
    pub const COLUMNS: &'static [&'static str] = &[
        "article_id",
        "version",
        "content_snapshot",
        "change_summary",
        "changed_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `kb_articles`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KBArticle {
    /// UUID
    pub category_id: Option<uuid::Uuid>,
    /// VARCHAR(500) NOT NULL
    pub title: String,
    /// VARCHAR(500) NOT NULL
    pub slug: String,
    /// TEXT
    pub content: Option<String>,
    /// TEXT
    pub summary: Option<String>,
    /// VARCHAR(30)
    pub article_type: Option<String>,
    /// VARCHAR(50)
    pub sop_code: Option<String>,
    /// VARCHAR(100)
    pub sop_owner_role: Option<String>,
    /// TEXT
    pub sop_trigger: Option<String>,
    /// JSONB
    pub sop_steps: Option<serde_json::Value>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(20)
    pub version: Option<String>,
    /// UUID
    pub previous_version_id: Option<uuid::Uuid>,
    /// TEXT
    pub version_notes: Option<String>,
    /// VARCHAR(30)
    pub access_level: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_private: bool,
    /// VARCHAR[]
    pub tags: Option<Vec<String>>,
    /// UUID[]
    pub related_article_ids: Option<Vec<uuid::Uuid>>,
    /// INTEGER
    pub view_count: Option<i32>,
    /// INTEGER
    pub helpful_votes: Option<i32>,
    /// INTEGER
    pub unhelpful_votes: Option<i32>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID
    pub reviewed_by_id: Option<uuid::Uuid>,
    /// UUID
    pub published_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub author_signed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(500)
    pub author_signature_url: Option<String>,
    /// UUID
    pub owner_signed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub owner_signed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(500)
    pub owner_signature_url: Option<String>,
    /// UUID
    pub source_pdf_attachment_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub pdf_import_status: Option<String>,
    /// TEXT
    pub pdf_import_error: Option<String>,
    /// VARCHAR(20)
    pub pdf_import_mode: Option<String>,
    /// INTEGER
    pub pdf_import_pages: Option<i32>,
    /// BOOLEAN
    pub pdf_import_truncated: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// BOOLEAN NOT NULL
    pub is_deleted: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl KBArticle {
    pub const TABLE: &'static str = "kb_articles";
    pub const COLUMNS: &'static [&'static str] = &[
        "category_id",
        "title",
        "slug",
        "content",
        "summary",
        "article_type",
        "sop_code",
        "sop_owner_role",
        "sop_trigger",
        "sop_steps",
        "status",
        "version",
        "previous_version_id",
        "version_notes",
        "access_level",
        "is_private",
        "tags",
        "related_article_ids",
        "view_count",
        "helpful_votes",
        "unhelpful_votes",
        "created_by_id",
        "reviewed_by_id",
        "published_by_id",
        "author_signed_at",
        "author_signature_url",
        "owner_signed_by_id",
        "owner_signed_at",
        "owner_signature_url",
        "source_pdf_attachment_id",
        "pdf_import_status",
        "pdf_import_error",
        "pdf_import_mode",
        "pdf_import_pages",
        "pdf_import_truncated",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `kb_categories`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KBCategory {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(255) NOT NULL
    pub slug: String,
    /// TEXT
    pub description: Option<String>,
    /// UUID
    pub parent_id: Option<uuid::Uuid>,
    /// INTEGER
    pub sort_order: Option<i32>,
    /// VARCHAR(100)
    pub icon: Option<String>,
    /// VARCHAR(30)
    pub access_level: Option<String>,
    /// VARCHAR[]
    pub allowed_departments: Option<Vec<String>>,
    /// VARCHAR[]
    pub allowed_roles: Option<Vec<String>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// BOOLEAN NOT NULL
    pub is_deleted: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl KBCategory {
    pub const TABLE: &'static str = "kb_categories";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "slug",
        "description",
        "parent_id",
        "sort_order",
        "icon",
        "access_level",
        "allowed_departments",
        "allowed_roles",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
