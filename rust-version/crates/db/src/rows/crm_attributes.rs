// Row structs of the crm_attributes tables: one struct per table, its columns in order.

/// Row of `crm_ai_field_runs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CRMAIFieldRun {
    /// VARCHAR(20) NOT NULL
    pub object_type: String,
    /// UUID NOT NULL
    pub record_id: uuid::Uuid,
    /// VARCHAR(64) NOT NULL
    pub attribute_key: String,
    /// JSONB
    pub value: Option<serde_json::Value>,
    /// NUMERIC(5, 2)
    pub confidence: Option<rust_decimal::Decimal>,
    /// TEXT
    pub reasoning: Option<String>,
    /// JSONB
    pub sources: Option<serde_json::Value>,
    /// VARCHAR(30)
    pub provider: Option<String>,
    /// VARCHAR(80)
    pub model: Option<String>,
    /// INTEGER NOT NULL
    pub prompt_tokens: i32,
    /// INTEGER NOT NULL
    pub completion_tokens: i32,
    /// TEXT
    pub error: Option<String>,
    /// UUID
    pub run_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CRMAIFieldRun {
    pub const TABLE: &'static str = "crm_ai_field_runs";
    pub const COLUMNS: &'static [&'static str] = &[
        "object_type",
        "record_id",
        "attribute_key",
        "value",
        "confidence",
        "reasoning",
        "sources",
        "provider",
        "model",
        "prompt_tokens",
        "completion_tokens",
        "error",
        "run_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `crm_attribute_defs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CRMAttributeDef {
    /// VARCHAR(20) NOT NULL
    pub object_type: String,
    /// VARCHAR(64) NOT NULL
    pub key: String,
    /// VARCHAR(200) NOT NULL
    pub label: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(24) NOT NULL
    pub field_type: String,
    /// JSONB
    pub options: Option<serde_json::Value>,
    /// BOOLEAN NOT NULL
    pub is_ai: bool,
    /// VARCHAR(20)
    pub ai_kind: Option<String>,
    /// JSONB
    pub ai_config: Option<serde_json::Value>,
    /// INTEGER NOT NULL
    pub sort_order: i32,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CRMAttributeDef {
    pub const TABLE: &'static str = "crm_attribute_defs";
    pub const COLUMNS: &'static [&'static str] = &[
        "object_type",
        "key",
        "label",
        "description",
        "field_type",
        "options",
        "is_ai",
        "ai_kind",
        "ai_config",
        "sort_order",
        "is_active",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}
