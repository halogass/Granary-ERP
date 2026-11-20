// Row structs of the crm_dedupe tables: one struct per table, its columns in order.

/// Row of `crm_duplicate_candidates`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CRMDuplicateCandidate {
    /// VARCHAR(20) NOT NULL
    pub object_type: String,
    /// UUID NOT NULL
    pub record_a_id: uuid::Uuid,
    /// UUID NOT NULL
    pub record_b_id: uuid::Uuid,
    /// NUMERIC(5, 2) NOT NULL
    pub score: rust_decimal::Decimal,
    /// JSONB
    pub reasons: Option<serde_json::Value>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// UUID
    pub resolved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub merged_into_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CRMDuplicateCandidate {
    pub const TABLE: &'static str = "crm_duplicate_candidates";
    pub const COLUMNS: &'static [&'static str] = &[
        "object_type",
        "record_a_id",
        "record_b_id",
        "score",
        "reasons",
        "status",
        "resolved_by_id",
        "resolved_at",
        "merged_into_id",
        "id",
        "created_at",
        "updated_at",
    ];
}
