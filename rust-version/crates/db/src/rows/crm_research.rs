// Row structs of the crm_research tables: one struct per table, its columns in order.

/// Row of `crm_research_people`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CRMResearchPerson {
    /// UUID NOT NULL
    pub organization_id: uuid::Uuid,
    /// UUID
    pub run_id: Option<uuid::Uuid>,
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// VARCHAR(200) NOT NULL
    pub name_key: String,
    /// VARCHAR(200)
    pub title: Option<String>,
    /// VARCHAR(1000)
    pub source_url: Option<String>,
    /// TEXT
    pub evidence: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub matched_contact_id: Option<uuid::Uuid>,
    /// UUID
    pub resolved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CRMResearchPerson {
    pub const TABLE: &'static str = "crm_research_people";
    pub const COLUMNS: &'static [&'static str] = &[
        "organization_id",
        "run_id",
        "name",
        "name_key",
        "title",
        "source_url",
        "evidence",
        "status",
        "contact_id",
        "matched_contact_id",
        "resolved_by_id",
        "resolved_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `crm_research_runs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CRMResearchRun {
    /// VARCHAR(20) NOT NULL
    pub object_type: String,
    /// UUID NOT NULL
    pub record_id: uuid::Uuid,
    /// VARCHAR(10) NOT NULL
    pub depth: String,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// TEXT
    pub summary: Option<String>,
    /// JSONB
    pub sources: Option<serde_json::Value>,
    /// JSONB
    pub news: Option<serde_json::Value>,
    /// JSONB
    pub channels: Option<serde_json::Value>,
    /// JSONB
    pub written: Option<serde_json::Value>,
    /// JSONB
    pub suggested: Option<serde_json::Value>,
    /// TEXT
    pub error: Option<String>,
    /// UUID
    pub requested_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CRMResearchRun {
    pub const TABLE: &'static str = "crm_research_runs";
    pub const COLUMNS: &'static [&'static str] = &[
        "object_type",
        "record_id",
        "depth",
        "status",
        "summary",
        "sources",
        "news",
        "channels",
        "written",
        "suggested",
        "error",
        "requested_by_id",
        "started_at",
        "finished_at",
        "id",
        "created_at",
        "updated_at",
    ];
}
