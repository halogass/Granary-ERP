// Row structs of the verification tables: one struct per table, its columns in order.

/// Row of `document_verification_views`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentVerificationView {
    /// UUID NOT NULL
    pub verification_id: uuid::Uuid,
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// VARCHAR(45)
    pub ip_address: Option<String>,
    /// TEXT
    pub user_agent: Option<String>,
    /// VARCHAR(30) NOT NULL
    pub outcome: String,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl DocumentVerificationView {
    pub const TABLE: &'static str = "document_verification_views";
    pub const COLUMNS: &'static [&'static str] = &[
        "verification_id",
        "user_id",
        "ip_address",
        "user_agent",
        "outcome",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `document_verifications`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentVerification {
    /// VARCHAR(50) NOT NULL
    pub entity_type: String,
    /// UUID NOT NULL
    pub entity_id: uuid::Uuid,
    /// INTEGER NOT NULL
    pub revision: i32,
    /// VARCHAR(100)
    pub document_number: Option<String>,
    /// VARCHAR(255)
    pub document_title: Option<String>,
    /// UUID NOT NULL
    pub token: uuid::Uuid,
    /// VARCHAR(16) NOT NULL
    pub short_code: String,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub issued_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub revoked_by_id: Option<uuid::Uuid>,
    /// TEXT
    pub revoke_reason: Option<String>,
    /// UUID
    pub superseded_by_id: Option<uuid::Uuid>,
    /// VARCHAR(30) NOT NULL
    pub view_scope: String,
    /// BOOLEAN NOT NULL
    pub allow_download: bool,
    /// BOOLEAN NOT NULL
    pub requires_login: bool,
    /// VARCHAR(64)
    pub document_hash: Option<String>,
    /// VARCHAR(500)
    pub pdf_object_key: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub rendered_at: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB NOT NULL
    pub facts: serde_json::Value,
    /// UUID
    pub approval_request_id: Option<uuid::Uuid>,
    /// UUID
    pub sign_document_id: Option<uuid::Uuid>,
    /// UUID
    pub issued_by_id: Option<uuid::Uuid>,
    /// INTEGER NOT NULL
    pub view_count: i32,
    /// TIMESTAMP WITH TIME ZONE
    pub last_viewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// VARCHAR(500) (migration 0152): the sealed outbound copy (DocMDP P=2) next to the unsealed master
    #[serde(default)]
    #[sqlx(default)]
    pub sealed_object_key: Option<String>,
    /// VARCHAR(64) (migration 0152): SHA-256 of the sealed copy
    #[serde(default)]
    #[sqlx(default)]
    pub sealed_hash: Option<String>,
    /// TIMESTAMP WITH TIME ZONE (migration 0152)
    #[serde(default)]
    #[sqlx(default)]
    pub sealed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(20) (migration 0152): pades-b-b | pades-b-t | pades-b-lta
    #[serde(default)]
    #[sqlx(default)]
    pub seal_level: Option<String>,
}

impl DocumentVerification {
    pub const TABLE: &'static str = "document_verifications";
    pub const COLUMNS: &'static [&'static str] = &[
        "entity_type",
        "entity_id",
        "revision",
        "document_number",
        "document_title",
        "token",
        "short_code",
        "status",
        "issued_at",
        "expires_at",
        "revoked_at",
        "revoked_by_id",
        "revoke_reason",
        "superseded_by_id",
        "view_scope",
        "allow_download",
        "requires_login",
        "document_hash",
        "pdf_object_key",
        "rendered_at",
        "facts",
        "approval_request_id",
        "sign_document_id",
        "issued_by_id",
        "view_count",
        "last_viewed_at",
        "id",
        "created_at",
        "updated_at",
        "sealed_object_key",
        "sealed_hash",
        "sealed_at",
        "seal_level",
    ];
}
