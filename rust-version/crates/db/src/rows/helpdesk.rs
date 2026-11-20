// Row structs of the helpdesk tables: one struct per table, its columns in order.

/// Row of `helpdesk_categories`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HelpdeskCategory {
    /// UUID NOT NULL
    pub team_id: uuid::Uuid,
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl HelpdeskCategory {
    pub const TABLE: &'static str = "helpdesk_categories";
    pub const COLUMNS: &'static [&'static str] = &[
        "team_id",
        "name",
        "description",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `helpdesk_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HelpdeskMessage {
    /// UUID NOT NULL
    pub ticket_id: uuid::Uuid,
    /// TEXT NOT NULL
    pub body: String,
    /// BOOLEAN NOT NULL
    pub is_internal: bool,
    /// VARCHAR(255)
    pub author_name: Option<String>,
    /// VARCHAR(255)
    pub author_email: Option<String>,
    /// UUID
    pub author_user_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl HelpdeskMessage {
    pub const TABLE: &'static str = "helpdesk_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "ticket_id",
        "body",
        "is_internal",
        "author_name",
        "author_email",
        "author_user_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `helpdesk_sla_policies`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HelpdeskSLAPolicy {
    /// UUID NOT NULL
    pub team_id: uuid::Uuid,
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// VARCHAR(20) NOT NULL
    pub priority: String,
    /// NUMERIC(5, 1) NOT NULL
    pub response_hours: rust_decimal::Decimal,
    /// NUMERIC(5, 1) NOT NULL
    pub resolution_hours: rust_decimal::Decimal,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl HelpdeskSLAPolicy {
    pub const TABLE: &'static str = "helpdesk_sla_policies";
    pub const COLUMNS: &'static [&'static str] = &[
        "team_id",
        "name",
        "priority",
        "response_hours",
        "resolution_hours",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `helpdesk_teams`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HelpdeskTeam {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// VARCHAR(255)
    pub email: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl HelpdeskTeam {
    pub const TABLE: &'static str = "helpdesk_teams";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "email",
        "description",
        "is_active",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `helpdesk_tickets`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HelpdeskTicket {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// VARCHAR(500) NOT NULL
    pub subject: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(20)
    pub priority: Option<String>,
    /// UUID
    pub team_id: Option<uuid::Uuid>,
    /// UUID
    pub category_id: Option<uuid::Uuid>,
    /// UUID
    pub sla_policy_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub customer_name: String,
    /// VARCHAR(255) NOT NULL
    pub customer_email: String,
    /// VARCHAR(255)
    pub customer_company: Option<String>,
    /// UUID
    pub assignee_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub public_token: uuid::Uuid,
    /// VARCHAR(30)
    pub source: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub response_deadline: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub resolution_deadline: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub first_responded_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub closed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BOOLEAN
    pub first_response_breached: Option<bool>,
    /// BOOLEAN
    pub resolution_breached: Option<bool>,
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

impl HelpdeskTicket {
    pub const TABLE: &'static str = "helpdesk_tickets";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "subject",
        "description",
        "status",
        "priority",
        "team_id",
        "category_id",
        "sla_policy_id",
        "customer_name",
        "customer_email",
        "customer_company",
        "assignee_id",
        "public_token",
        "source",
        "response_deadline",
        "resolution_deadline",
        "first_responded_at",
        "resolved_at",
        "closed_at",
        "first_response_breached",
        "resolution_breached",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
