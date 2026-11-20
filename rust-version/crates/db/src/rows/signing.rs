// Row structs of the signing tables: one struct per table, its columns in order.

/// Row of `sign_audit_logs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignAuditLog {
    /// UUID NOT NULL
    pub document_id: uuid::Uuid,
    /// UUID
    pub signer_id: Option<uuid::Uuid>,
    /// VARCHAR(50) NOT NULL
    pub action: String,
    /// VARCHAR(500)
    pub description: Option<String>,
    /// VARCHAR(45)
    pub ip_address: Option<String>,
    /// TEXT
    pub user_agent: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SignAuditLog {
    pub const TABLE: &'static str = "sign_audit_logs";
    pub const COLUMNS: &'static [&'static str] = &[
        "document_id",
        "signer_id",
        "action",
        "description",
        "ip_address",
        "user_agent",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `sign_documents`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignDocument {
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(30)
    pub source_type: Option<String>,
    /// UUID
    pub source_id: Option<uuid::Uuid>,
    /// VARCHAR(500)
    pub original_file_url: Option<String>,
    /// VARCHAR(500)
    pub signed_file_url: Option<String>,
    /// VARCHAR(500)
    pub certificate_url: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub certificate_generated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub sealed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(95)
    pub seal_fingerprint: Option<String>,
    /// VARCHAR(64)
    pub document_hash: Option<String>,
    /// VARCHAR(10)
    pub signature_level: Option<String>,
    /// VARCHAR(20)
    pub seal_level: Option<String>,
    /// VARCHAR(50)
    pub jurisdictions: Option<String>,
    /// TEXT
    pub message: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
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

impl SignDocument {
    pub const TABLE: &'static str = "sign_documents";
    pub const COLUMNS: &'static [&'static str] = &[
        "title",
        "status",
        "source_type",
        "source_id",
        "original_file_url",
        "signed_file_url",
        "certificate_url",
        "certificate_generated_at",
        "sealed_at",
        "seal_fingerprint",
        "document_hash",
        "signature_level",
        "seal_level",
        "jurisdictions",
        "message",
        "expires_at",
        "completed_at",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `sign_fields`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignField {
    /// UUID NOT NULL
    pub document_id: uuid::Uuid,
    /// UUID
    pub signer_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub field_type: String,
    /// INTEGER
    pub page_number: Option<i32>,
    /// NUMERIC(8, 2)
    pub x: Option<rust_decimal::Decimal>,
    /// NUMERIC(8, 2)
    pub y: Option<rust_decimal::Decimal>,
    /// NUMERIC(8, 2)
    pub width: Option<rust_decimal::Decimal>,
    /// NUMERIC(8, 2)
    pub height: Option<rust_decimal::Decimal>,
    /// BOOLEAN
    pub is_required: Option<bool>,
    /// VARCHAR(255)
    pub label: Option<String>,
    /// TEXT
    pub value: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SignField {
    pub const TABLE: &'static str = "sign_fields";
    pub const COLUMNS: &'static [&'static str] = &[
        "document_id",
        "signer_id",
        "field_type",
        "page_number",
        "x",
        "y",
        "width",
        "height",
        "is_required",
        "label",
        "value",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `sign_signers`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignSigner {
    /// UUID NOT NULL
    pub document_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// INTEGER NOT NULL
    pub sign_order: i32,
    /// VARCHAR(20)
    pub role: Option<String>,
    /// UUID NOT NULL
    pub token: uuid::Uuid,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub viewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub signed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub declined_reason: Option<String>,
    /// VARCHAR(45)
    pub ip_address: Option<String>,
    /// TEXT
    pub user_agent: Option<String>,
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub signer_type: Option<String>,
    /// UUID
    pub partner_account_id: Option<uuid::Uuid>,
    /// TEXT
    pub signature_data: Option<String>,
    /// TEXT
    pub initials_data: Option<String>,
    /// VARCHAR(50)
    pub sign_reason: Option<String>,
    /// VARCHAR(20)
    pub consent_version: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub consent_agreed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(64)
    pub consent_text_sha256: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SignSigner {
    pub const TABLE: &'static str = "sign_signers";
    pub const COLUMNS: &'static [&'static str] = &[
        "document_id",
        "name",
        "email",
        "sign_order",
        "role",
        "token",
        "status",
        "sent_at",
        "viewed_at",
        "signed_at",
        "declined_reason",
        "ip_address",
        "user_agent",
        "user_id",
        "signer_type",
        "partner_account_id",
        "signature_data",
        "initials_data",
        "sign_reason",
        "consent_version",
        "consent_agreed_at",
        "consent_text_sha256",
        "id",
        "created_at",
        "updated_at",
    ];
}
