// Row structs of the rnd tables: one struct per table, its columns in order.

/// Row of `gitlab_configs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitLabConfig {
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// VARCHAR(500) NOT NULL
    pub url: String,
    /// VARCHAR(8)
    pub api_version: Option<String>,
    /// TEXT
    pub access_token_encrypted: Option<String>,
    /// VARCHAR(200)
    pub webhook_secret: Option<String>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// BOOLEAN
    pub verify_ssl: Option<bool>,
    /// VARCHAR(200)
    pub instance_name: Option<String>,
    /// VARCHAR(50)
    pub instance_version: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl GitLabConfig {
    pub const TABLE: &'static str = "gitlab_configs";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "url",
        "api_version",
        "access_token_encrypted",
        "webhook_secret",
        "is_active",
        "verify_ssl",
        "instance_name",
        "instance_version",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `gitlab_pipeline_runs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitLabPipelineRun {
    /// UUID NOT NULL
    pub repository_id: uuid::Uuid,
    /// INTEGER NOT NULL
    pub gitlab_pipeline_id: i32,
    /// VARCHAR(32) NOT NULL
    pub status: String,
    #[sqlx(rename = "ref")]
    #[serde(rename = "ref")]
    /// VARCHAR(200)
    pub ref_: Option<String>,
    /// VARCHAR(40)
    pub sha: Option<String>,
    /// VARCHAR(50)
    pub source: Option<String>,
    /// VARCHAR(200)
    pub triggered_by: Option<String>,
    /// VARCHAR(500)
    pub web_url: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// INTEGER
    pub duration_seconds: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl GitLabPipelineRun {
    pub const TABLE: &'static str = "gitlab_pipeline_runs";
    pub const COLUMNS: &'static [&'static str] = &[
        "repository_id",
        "gitlab_pipeline_id",
        "status",
        "ref",
        "sha",
        "source",
        "triggered_by",
        "web_url",
        "started_at",
        "finished_at",
        "duration_seconds",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `gitlab_repositories`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitLabRepository {
    /// UUID NOT NULL
    pub gitlab_config_id: uuid::Uuid,
    /// UUID
    pub rnd_project_id: Option<uuid::Uuid>,
    /// INTEGER NOT NULL
    pub gitlab_project_id: i32,
    /// VARCHAR(500) NOT NULL
    pub full_path: String,
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(100)
    pub default_branch: Option<String>,
    /// VARCHAR(500)
    pub web_url: Option<String>,
    /// INTEGER
    pub webhook_id: Option<i32>,
    /// VARCHAR(32)
    pub last_pipeline_status: Option<String>,
    /// VARCHAR(200)
    pub last_pipeline_ref: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_pipeline_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl GitLabRepository {
    pub const TABLE: &'static str = "gitlab_repositories";
    pub const COLUMNS: &'static [&'static str] = &[
        "gitlab_config_id",
        "rnd_project_id",
        "gitlab_project_id",
        "full_path",
        "name",
        "description",
        "default_branch",
        "web_url",
        "webhook_id",
        "last_pipeline_status",
        "last_pipeline_ref",
        "last_pipeline_at",
        "is_active",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `gitlab_webhook_events`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitLabWebhookEvent {
    /// UUID NOT NULL
    pub repository_id: uuid::Uuid,
    /// VARCHAR(50) NOT NULL
    pub event_type: String,
    /// INTEGER
    pub object_id: Option<i32>,
    /// INTEGER
    pub object_iid: Option<i32>,
    /// VARCHAR(500)
    pub object_title: Option<String>,
    /// VARCHAR(50)
    pub object_state: Option<String>,
    /// VARCHAR(50)
    pub object_action: Option<String>,
    /// VARCHAR(200)
    pub author_name: Option<String>,
    /// VARCHAR(200)
    pub author_username: Option<String>,
    /// VARCHAR(500)
    pub web_url: Option<String>,
    /// UUID
    pub linked_backlog_item_id: Option<uuid::Uuid>,
    /// JSONB
    pub raw_payload: Option<serde_json::Value>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl GitLabWebhookEvent {
    pub const TABLE: &'static str = "gitlab_webhook_events";
    pub const COLUMNS: &'static [&'static str] = &[
        "repository_id",
        "event_type",
        "object_id",
        "object_iid",
        "object_title",
        "object_state",
        "object_action",
        "author_name",
        "author_username",
        "web_url",
        "linked_backlog_item_id",
        "raw_payload",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `rnd_backlog_items`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RDBacklogItem {
    /// UUID NOT NULL
    pub rnd_project_id: uuid::Uuid,
    /// UUID
    pub sprint_id: Option<uuid::Uuid>,
    /// VARCHAR(500) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(32)
    pub item_type: Option<String>,
    /// VARCHAR(32)
    pub status: Option<String>,
    /// VARCHAR(16)
    pub priority: Option<String>,
    /// INTEGER
    pub story_points: Option<i32>,
    /// UUID
    pub assignee_id: Option<uuid::Uuid>,
    /// UUID
    pub reporter_id: Option<uuid::Uuid>,
    /// INTEGER
    pub gitlab_issue_id: Option<i32>,
    /// INTEGER
    pub gitlab_issue_iid: Option<i32>,
    /// VARCHAR(500)
    pub gitlab_issue_url: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub gitlab_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB
    pub labels: Option<serde_json::Value>,
    /// TEXT
    pub acceptance_criteria: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// INTEGER
    pub board_rank: Option<i32>,
    /// UUID
    pub epic_id: Option<uuid::Uuid>,
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

impl RDBacklogItem {
    pub const TABLE: &'static str = "rnd_backlog_items";
    pub const COLUMNS: &'static [&'static str] = &[
        "rnd_project_id",
        "sprint_id",
        "title",
        "description",
        "item_type",
        "status",
        "priority",
        "story_points",
        "assignee_id",
        "reporter_id",
        "gitlab_issue_id",
        "gitlab_issue_iid",
        "gitlab_issue_url",
        "gitlab_synced_at",
        "labels",
        "acceptance_criteria",
        "resolved_at",
        "board_rank",
        "epic_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `rnd_projects`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RDProject {
    /// VARCHAR(32) NOT NULL
    pub code: String,
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(32)
    pub status: Option<String>,
    /// VARCHAR(64)
    pub product_ref: Option<String>,
    /// UUID
    pub hardware_product_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_engineer_id: Option<uuid::Uuid>,
    /// VARCHAR(500)
    pub primary_repo_url: Option<String>,
    /// VARCHAR(50)
    pub latest_version: Option<String>,
    /// TEXT
    pub roadmap_notes: Option<String>,
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

impl RDProject {
    pub const TABLE: &'static str = "rnd_projects";
    pub const COLUMNS: &'static [&'static str] = &[
        "code",
        "name",
        "description",
        "status",
        "product_ref",
        "hardware_product_id",
        "lead_engineer_id",
        "primary_repo_url",
        "latest_version",
        "roadmap_notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `rnd_releases`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RDRelease {
    /// UUID NOT NULL
    pub rnd_project_id: uuid::Uuid,
    /// VARCHAR(50) NOT NULL
    pub version: String,
    /// VARCHAR(200)
    pub release_name: Option<String>,
    /// VARCHAR(32)
    pub status: Option<String>,
    /// DATE
    pub release_date: Option<chrono::NaiveDate>,
    /// TEXT
    pub release_notes: Option<String>,
    /// JSONB
    pub changelog: Option<serde_json::Value>,
    /// VARCHAR(500)
    pub gitlab_tag_url: Option<String>,
    /// VARCHAR(100)
    pub gitlab_tag_name: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl RDRelease {
    pub const TABLE: &'static str = "rnd_releases";
    pub const COLUMNS: &'static [&'static str] = &[
        "rnd_project_id",
        "version",
        "release_name",
        "status",
        "release_date",
        "release_notes",
        "changelog",
        "gitlab_tag_url",
        "gitlab_tag_name",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `rnd_sprints`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RDSprint {
    /// UUID NOT NULL
    pub rnd_project_id: uuid::Uuid,
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// TEXT
    pub goal: Option<String>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// VARCHAR(32)
    pub status: Option<String>,
    /// INTEGER
    pub velocity_planned: Option<i32>,
    /// INTEGER
    pub velocity_achieved: Option<i32>,
    /// TEXT
    pub retrospective_notes: Option<String>,
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

impl RDSprint {
    pub const TABLE: &'static str = "rnd_sprints";
    pub const COLUMNS: &'static [&'static str] = &[
        "rnd_project_id",
        "name",
        "goal",
        "start_date",
        "end_date",
        "status",
        "velocity_planned",
        "velocity_achieved",
        "retrospective_notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
