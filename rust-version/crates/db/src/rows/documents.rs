// Row structs of the documents tables: one struct per table, its columns in order.

/// Row of `nary_cost_sheets`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryCostSheet {
    /// UUID
    pub document_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// JSONB NOT NULL
    pub content: serde_json::Value,
    /// VARCHAR(3) NOT NULL
    pub currency: String,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// VARCHAR(80)
    pub model: Option<String>,
    /// UUID
    pub run_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryCostSheet {
    pub const TABLE: &'static str = "nary_cost_sheets";
    pub const COLUMNS: &'static [&'static str] = &[
        "document_id",
        "deal_id",
        "content",
        "currency",
        "status",
        "model",
        "run_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_rate_card`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryRateCard {
    /// VARCHAR(80) NOT NULL
    pub role: String,
    /// VARCHAR(10) NOT NULL
    pub seniority: String,
    /// NUMERIC(18, 2) NOT NULL
    pub cost_base: rust_decimal::Decimal,
    /// TEXT
    pub source: Option<String>,
    /// DATE NOT NULL
    pub valid_from: chrono::NaiveDate,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// BOOLEAN NOT NULL
    pub proposed_by_nary: bool,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryRateCard {
    pub const TABLE: &'static str = "nary_rate_card";
    pub const COLUMNS: &'static [&'static str] = &[
        "role",
        "seniority",
        "cost_base",
        "source",
        "valid_from",
        "status",
        "proposed_by_nary",
        "approved_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `sales_document_assets`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SalesDocumentAsset {
    /// UUID
    pub document_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub kind: String,
    /// VARCHAR(255)
    pub filename: Option<String>,
    /// VARCHAR(80)
    pub mime: Option<String>,
    /// VARCHAR(400)
    pub storage_key: Option<String>,
    /// VARCHAR(500)
    pub caption: Option<String>,
    /// JSONB
    pub scene: Option<serde_json::Value>,
    /// TEXT
    pub code: Option<String>,
    /// VARCHAR(80)
    pub model_name: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// JSONB NOT NULL
    pub checks: serde_json::Value,
    /// INTEGER NOT NULL
    pub attempts: i32,
    /// BOOLEAN NOT NULL
    pub promoted: bool,
    /// TEXT
    pub error: Option<String>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SalesDocumentAsset {
    pub const TABLE: &'static str = "sales_document_assets";
    pub const COLUMNS: &'static [&'static str] = &[
        "document_id",
        "kind",
        "filename",
        "mime",
        "storage_key",
        "caption",
        "scene",
        "code",
        "model_name",
        "status",
        "checks",
        "attempts",
        "promoted",
        "error",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `sales_document_blocks`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SalesDocumentBlock {
    /// UUID NOT NULL
    pub document_id: uuid::Uuid,
    /// VARCHAR(40) NOT NULL
    pub section: String,
    /// INTEGER NOT NULL
    pub position: i32,
    /// VARCHAR(20) NOT NULL
    pub block_type: String,
    /// JSONB NOT NULL
    pub data: serde_json::Value,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SalesDocumentBlock {
    pub const TABLE: &'static str = "sales_document_blocks";
    pub const COLUMNS: &'static [&'static str] = &[
        "document_id",
        "section",
        "position",
        "block_type",
        "data",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `sales_document_quotations`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SalesDocumentQuotation {
    /// UUID NOT NULL
    pub document_id: uuid::Uuid,
    /// UUID NOT NULL
    pub quotation_id: uuid::Uuid,
    /// INTEGER NOT NULL
    pub position: i32,
    /// VARCHAR(120)
    pub label: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SalesDocumentQuotation {
    pub const TABLE: &'static str = "sales_document_quotations";
    pub const COLUMNS: &'static [&'static str] = &[
        "document_id",
        "quotation_id",
        "position",
        "label",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `sales_documents`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SalesDocument {
    /// VARCHAR(30) NOT NULL
    pub doc_type: String,
    /// VARCHAR(60) NOT NULL
    pub number: String,
    /// VARCHAR(80)
    pub full_number: Option<String>,
    /// INTEGER NOT NULL
    pub revision: i32,
    /// VARCHAR(60)
    pub revision_group: Option<String>,
    /// UUID
    pub previous_version_id: Option<uuid::Uuid>,
    /// TEXT
    pub revision_reason: Option<String>,
    /// VARCHAR(300) NOT NULL
    pub title: String,
    /// VARCHAR(500)
    pub subtitle: Option<String>,
    /// VARCHAR(5) NOT NULL
    pub language: String,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub organization_id: Option<uuid::Uuid>,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// INTEGER NOT NULL
    pub validity_days: i32,
    /// VARCHAR(200)
    pub classification: Option<String>,
    /// JSONB NOT NULL
    pub cover: serde_json::Value,
    /// DATE
    pub issue_date: Option<chrono::NaiveDate>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(20)
    pub origin: Option<String>,
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

impl SalesDocument {
    pub const TABLE: &'static str = "sales_documents";
    pub const COLUMNS: &'static [&'static str] = &[
        "doc_type",
        "number",
        "full_number",
        "revision",
        "revision_group",
        "previous_version_id",
        "revision_reason",
        "title",
        "subtitle",
        "language",
        "deal_id",
        "organization_id",
        "contact_id",
        "status",
        "validity_days",
        "classification",
        "cover",
        "issue_date",
        "created_by_id",
        "sent_at",
        "origin",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
