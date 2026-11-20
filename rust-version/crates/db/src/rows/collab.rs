// Row structs of the collab tables: one struct per table, its columns in order.

/// Row of `board_columns`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BoardColumn {
    /// VARCHAR(20) NOT NULL
    pub scope_type: String,
    /// VARCHAR(36) NOT NULL
    pub scope_id: String,
    /// VARCHAR(40) NOT NULL
    pub key: String,
    /// VARCHAR(60) NOT NULL
    pub label: String,
    /// VARCHAR(16)
    pub color: Option<String>,
    /// INTEGER
    pub sort_order: Option<i32>,
    /// INTEGER
    pub wip_limit: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl BoardColumn {
    pub const TABLE: &'static str = "board_columns";
    pub const COLUMNS: &'static [&'static str] = &[
        "scope_type",
        "scope_id",
        "key",
        "label",
        "color",
        "sort_order",
        "wip_limit",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `epics`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Epic {
    /// VARCHAR(20) NOT NULL
    pub scope_type: String,
    /// VARCHAR(36) NOT NULL
    pub scope_id: String,
    /// VARCHAR(300) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(16)
    pub color: Option<String>,
    /// VARCHAR(20)
    pub status: Option<String>,
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

impl Epic {
    pub const TABLE: &'static str = "epics";
    pub const COLUMNS: &'static [&'static str] = &[
        "scope_type",
        "scope_id",
        "title",
        "description",
        "color",
        "status",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `issue_activity`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IssueActivity {
    /// VARCHAR(40) NOT NULL
    pub entity_type: String,
    /// VARCHAR(36) NOT NULL
    pub entity_id: String,
    /// UUID
    pub actor_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub verb: String,
    /// VARCHAR(60)
    pub field: Option<String>,
    /// VARCHAR(500)
    pub old_value: Option<String>,
    /// VARCHAR(500)
    pub new_value: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl IssueActivity {
    pub const TABLE: &'static str = "issue_activity";
    pub const COLUMNS: &'static [&'static str] = &[
        "entity_type",
        "entity_id",
        "actor_id",
        "verb",
        "field",
        "old_value",
        "new_value",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `issue_comments`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IssueComment {
    /// VARCHAR(40) NOT NULL
    pub entity_type: String,
    /// VARCHAR(36) NOT NULL
    pub entity_id: String,
    /// UUID
    pub author_id: Option<uuid::Uuid>,
    /// TEXT NOT NULL
    pub body: String,
    /// JSONB
    pub mentions: Option<serde_json::Value>,
    /// TIMESTAMP WITH TIME ZONE
    pub edited_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl IssueComment {
    pub const TABLE: &'static str = "issue_comments";
    pub const COLUMNS: &'static [&'static str] = &[
        "entity_type",
        "entity_id",
        "author_id",
        "body",
        "mentions",
        "edited_at",
        "id",
        "created_at",
        "updated_at",
    ];
}
