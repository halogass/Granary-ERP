// Row structs of the attachment tables: one struct per table, its columns in order.

/// Row of `attachments`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    /// VARCHAR(30) NOT NULL
    pub entity_type: String,
    /// VARCHAR(36) NOT NULL
    pub entity_id: String,
    /// UUID
    pub uploaded_by_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub file_name: String,
    /// INTEGER NOT NULL
    pub file_size: i32,
    /// VARCHAR(100) NOT NULL
    pub mime_type: String,
    /// VARCHAR(500) NOT NULL
    pub object_key: String,
    /// VARCHAR(500) NOT NULL
    pub url: String,
    /// VARCHAR(20) NOT NULL
    pub file_type: String,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Attachment {
    pub const TABLE: &'static str = "attachments";
    pub const COLUMNS: &'static [&'static str] = &[
        "entity_type",
        "entity_id",
        "uploaded_by_id",
        "file_name",
        "file_size",
        "mime_type",
        "object_key",
        "url",
        "file_type",
        "id",
        "created_at",
        "updated_at",
    ];
}
