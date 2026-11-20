// Row structs of the project tables: one struct per table, its columns in order.

/// Row of `project_members`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectMember {
    /// UUID NOT NULL
    pub project_id: uuid::Uuid,
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(50)
    pub role: Option<String>,
    /// INTEGER
    pub allocation_percentage: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ProjectMember {
    pub const TABLE: &'static str = "project_members";
    pub const COLUMNS: &'static [&'static str] = &[
        "project_id",
        "user_id",
        "role",
        "allocation_percentage",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `project_milestones`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectMilestone {
    /// UUID NOT NULL
    pub project_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// DATE NOT NULL
    pub due_date: chrono::NaiveDate,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// BOOLEAN
    pub triggers_invoice: Option<bool>,
    /// NUMERIC(5, 2)
    pub invoice_percentage: Option<rust_decimal::Decimal>,
    /// UUID
    pub invoice_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub invoice_triggered_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub achieved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub notes: Option<String>,
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

impl ProjectMilestone {
    pub const TABLE: &'static str = "project_milestones";
    pub const COLUMNS: &'static [&'static str] = &[
        "project_id",
        "name",
        "description",
        "due_date",
        "status",
        "triggers_invoice",
        "invoice_percentage",
        "invoice_id",
        "invoice_triggered_at",
        "achieved_at",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `project_phases`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectPhase {
    /// UUID NOT NULL
    pub project_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// INTEGER
    pub sort_order: Option<i32>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// TEXT
    pub description: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ProjectPhase {
    pub const TABLE: &'static str = "project_phases";
    pub const COLUMNS: &'static [&'static str] = &[
        "project_id",
        "name",
        "sort_order",
        "status",
        "start_date",
        "end_date",
        "description",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `project_tasks`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectTask {
    /// UUID NOT NULL
    pub project_id: uuid::Uuid,
    /// UUID
    pub phase_id: Option<uuid::Uuid>,
    /// UUID
    pub parent_task_id: Option<uuid::Uuid>,
    /// VARCHAR(500) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(20)
    pub priority: Option<String>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// DATE
    pub due_date: Option<chrono::NaiveDate>,
    /// NUMERIC(6, 2)
    pub estimated_hours: Option<rust_decimal::Decimal>,
    /// NUMERIC(6, 2)
    pub actual_hours: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub sort_order: Option<i32>,
    /// VARCHAR[]
    pub tags: Option<Vec<String>>,
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

impl ProjectTask {
    pub const TABLE: &'static str = "project_tasks";
    pub const COLUMNS: &'static [&'static str] = &[
        "project_id",
        "phase_id",
        "parent_task_id",
        "title",
        "description",
        "status",
        "priority",
        "assigned_to_id",
        "created_by_id",
        "due_date",
        "estimated_hours",
        "actual_hours",
        "sort_order",
        "tags",
        "epic_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `projects`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Project {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(50)
    pub project_number: Option<String>,
    /// VARCHAR(50) NOT NULL
    pub project_type: String,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub contract_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_contact_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub pm_id: Option<uuid::Uuid>,
    /// UUID
    pub tech_lead_id: Option<uuid::Uuid>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// NUMERIC(18, 2)
    pub budget: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 2)
    pub cost_to_date: Option<rust_decimal::Decimal>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// DATE
    pub actual_start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub actual_end_date: Option<chrono::NaiveDate>,
    /// TEXT
    pub description: Option<String>,
    /// TEXT
    pub objectives: Option<String>,
    /// JSONB
    pub deliverables: Option<serde_json::Value>,
    /// JSONB
    pub risks: Option<serde_json::Value>,
    /// TEXT
    pub notes: Option<String>,
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

impl Project {
    pub const TABLE: &'static str = "projects";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "project_number",
        "project_type",
        "deal_id",
        "contract_id",
        "customer_org_id",
        "customer_contact_id",
        "status",
        "pm_id",
        "tech_lead_id",
        "created_by_id",
        "budget",
        "currency",
        "cost_to_date",
        "start_date",
        "end_date",
        "actual_start_date",
        "actual_end_date",
        "description",
        "objectives",
        "deliverables",
        "risks",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `time_logs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TimeLog {
    /// UUID
    pub task_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub project_id: uuid::Uuid,
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// DATE NOT NULL
    pub date: chrono::NaiveDate,
    /// NUMERIC(5, 2) NOT NULL
    pub hours: rust_decimal::Decimal,
    /// TEXT
    pub description: Option<String>,
    /// BOOLEAN
    pub is_billable: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TimeLog {
    pub const TABLE: &'static str = "time_logs";
    pub const COLUMNS: &'static [&'static str] = &[
        "task_id",
        "project_id",
        "user_id",
        "date",
        "hours",
        "description",
        "is_billable",
        "id",
        "created_at",
        "updated_at",
    ];
}
