// Row structs of the email_signature tables: one struct per table, its columns in order.

/// Row of `email_signature_assets`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmailSignatureAsset {
    /// VARCHAR(255) NOT NULL
    pub filename: String,
    /// VARCHAR(100) NOT NULL
    pub mime_type: String,
    /// VARCHAR(500) NOT NULL
    pub object_key: String,
    /// INTEGER
    pub file_size: Option<i32>,
    /// VARCHAR(500)
    pub source_url: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl EmailSignatureAsset {
    pub const TABLE: &'static str = "email_signature_assets";
    pub const COLUMNS: &'static [&'static str] = &[
        "filename",
        "mime_type",
        "object_key",
        "file_size",
        "source_url",
        "id",
        "created_at",
        "updated_at",
    ];
}
