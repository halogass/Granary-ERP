// Row structs of the ai tables: one struct per table, its columns in order.

/// Row of `ai_conversations`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AIConversation {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(300)
    pub title: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AIConversation {
    pub const TABLE: &'static str = "ai_conversations";
    pub const COLUMNS: &'static [&'static str] =
        &["user_id", "title", "id", "created_at", "updated_at"];
}

/// Row of `ai_knowledge_chunks`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AIKnowledgeChunk {
    /// UUID NOT NULL
    pub doc_id: uuid::Uuid,
    /// INTEGER NOT NULL
    pub chunk_index: i32,
    /// TEXT NOT NULL
    pub content: String,
    /// VECTOR(768)
    pub embedding: Option<crate::rows::Vector>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AIKnowledgeChunk {
    pub const TABLE: &'static str = "ai_knowledge_chunks";
    pub const COLUMNS: &'static [&'static str] = &[
        "doc_id",
        "chunk_index",
        "content",
        "embedding",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `ai_knowledge_docs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AIKnowledgeDoc {
    /// VARCHAR(300) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(10) NOT NULL
    pub source_type: String,
    /// TEXT
    pub content: Option<String>,
    /// VARCHAR(300)
    pub original_filename: Option<String>,
    /// VARCHAR(500)
    pub original_object_key: Option<String>,
    /// INTEGER
    pub original_size: Option<i32>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// TEXT
    pub error: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// BOOLEAN NOT NULL
    pub customer_safe: bool,
    /// INTEGER NOT NULL
    pub chunk_count: i32,
    /// INTEGER NOT NULL
    pub use_count: i32,
    /// TIMESTAMP WITH TIME ZONE
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// VARCHAR(120)
    pub system_key: Option<String>,
    /// VARCHAR(64)
    pub content_hash: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AIKnowledgeDoc {
    pub const TABLE: &'static str = "ai_knowledge_docs";
    pub const COLUMNS: &'static [&'static str] = &[
        "title",
        "description",
        "source_type",
        "content",
        "original_filename",
        "original_object_key",
        "original_size",
        "status",
        "error",
        "is_active",
        "customer_safe",
        "chunk_count",
        "use_count",
        "last_used_at",
        "created_by_id",
        "system_key",
        "content_hash",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `ai_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AIMessage {
    /// UUID NOT NULL
    pub conversation_id: uuid::Uuid,
    /// VARCHAR(20) NOT NULL
    pub role: String,
    /// TEXT
    pub content: Option<String>,
    /// JSONB
    pub tool_calls: Option<serde_json::Value>,
    /// JSONB
    pub citations: Option<serde_json::Value>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AIMessage {
    pub const TABLE: &'static str = "ai_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "conversation_id",
        "role",
        "content",
        "tool_calls",
        "citations",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `ai_usage_log`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AIUsageLog {
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// VARCHAR(40) NOT NULL
    pub feature: String,
    /// VARCHAR(30)
    pub provider: Option<String>,
    /// VARCHAR(80)
    pub model: Option<String>,
    /// INTEGER
    pub prompt_tokens: Option<i32>,
    /// INTEGER
    pub completion_tokens: Option<i32>,
    /// INTEGER
    pub cache_read_tokens: Option<i32>,
    /// INTEGER NOT NULL
    pub cache_write_1h_tokens: i32,
    /// INTEGER
    pub cache_write_tokens: Option<i32>,
    /// NUMERIC(12, 6)
    pub est_cost: Option<rust_decimal::Decimal>,
    /// BOOLEAN NOT NULL
    pub cost_known: bool,
    /// UUID
    pub run_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AIUsageLog {
    pub const TABLE: &'static str = "ai_usage_log";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "feature",
        "provider",
        "model",
        "prompt_tokens",
        "completion_tokens",
        "cache_read_tokens",
        "cache_write_1h_tokens",
        "cache_write_tokens",
        "est_cost",
        "cost_known",
        "run_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `crm_embeddings`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CRMEmbedding {
    /// VARCHAR(30) NOT NULL
    pub object_type: String,
    /// UUID NOT NULL
    pub record_id: uuid::Uuid,
    /// TEXT NOT NULL
    pub label: String,
    /// VECTOR(768)
    pub embedding: Option<crate::rows::Vector>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CRMEmbedding {
    pub const TABLE: &'static str = "crm_embeddings";
    pub const COLUMNS: &'static [&'static str] = &[
        "object_type",
        "record_id",
        "label",
        "embedding",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `kb_embeddings`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KBEmbedding {
    /// UUID NOT NULL
    pub article_id: uuid::Uuid,
    /// INTEGER NOT NULL
    pub chunk_index: i32,
    /// TEXT NOT NULL
    pub content: String,
    /// VECTOR(768)
    pub embedding: Option<crate::rows::Vector>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl KBEmbedding {
    pub const TABLE: &'static str = "kb_embeddings";
    pub const COLUMNS: &'static [&'static str] = &[
        "article_id",
        "chunk_index",
        "content",
        "embedding",
        "id",
        "created_at",
        "updated_at",
    ];
}
