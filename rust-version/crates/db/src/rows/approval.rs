// Row structs of the approval tables: one struct per table, its columns in order.

/// Row of `approval_actions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApprovalAction {
    /// UUID NOT NULL
    pub request_id: uuid::Uuid,
    /// INTEGER
    pub step_seq: Option<i32>,
    /// UUID
    pub actor_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub action: String,
    /// TEXT
    pub comment: Option<String>,
    /// VARCHAR(45)
    pub ip_address: Option<String>,
    /// TEXT
    pub user_agent: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ApprovalAction {
    pub const TABLE: &'static str = "approval_actions";
    pub const COLUMNS: &'static [&'static str] = &[
        "request_id",
        "step_seq",
        "actor_id",
        "action",
        "comment",
        "ip_address",
        "user_agent",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `approval_policies`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApprovalPolicy {
    /// VARCHAR(50) NOT NULL
    pub entity_type: String,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(100)
    pub sop_reference: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// INTEGER NOT NULL
    pub version: i32,
    /// JSONB NOT NULL
    pub preconditions: serde_json::Value,
    /// VARCHAR(40) NOT NULL
    pub signature_flow: String,
    /// VARCHAR(100)
    pub signer_role: Option<String>,
    /// INTEGER
    pub sign_deadline_days: Option<i32>,
    /// INTEGER
    pub max_extension_days: Option<i32>,
    /// BOOLEAN NOT NULL
    pub qr_enabled: bool,
    /// VARCHAR(30) NOT NULL
    pub view_scope: String,
    /// BOOLEAN NOT NULL
    pub allow_download: bool,
    /// BOOLEAN NOT NULL
    pub requires_login: bool,
    /// INTEGER
    pub token_ttl_days: Option<i32>,
    /// INTEGER
    pub sla_hours: Option<i32>,
    /// NUMERIC(18, 2)
    pub whatsapp_escalate_above_base: Option<rust_decimal::Decimal>,
    /// UUID
    pub updated_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ApprovalPolicy {
    pub const TABLE: &'static str = "approval_policies";
    pub const COLUMNS: &'static [&'static str] = &[
        "entity_type",
        "name",
        "description",
        "sop_reference",
        "is_active",
        "version",
        "preconditions",
        "signature_flow",
        "signer_role",
        "sign_deadline_days",
        "max_extension_days",
        "qr_enabled",
        "view_scope",
        "allow_download",
        "requires_login",
        "token_ttl_days",
        "sla_hours",
        "whatsapp_escalate_above_base",
        "updated_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `approval_policy_steps`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApprovalPolicyStep {
    /// UUID NOT NULL
    pub policy_id: uuid::Uuid,
    /// INTEGER NOT NULL
    pub sequence: i32,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(20) NOT NULL
    pub approver_mode: String,
    /// VARCHAR(100)
    pub role_slug: Option<String>,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// VARCHAR(100)
    pub snapshot_user_key: Option<String>,
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// JSONB
    pub condition: Option<serde_json::Value>,
    /// INTEGER
    pub sla_hours: Option<i32>,
    /// BOOLEAN NOT NULL
    pub is_notify_only: bool,
    /// BOOLEAN NOT NULL
    pub allow_self_approval: bool,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ApprovalPolicyStep {
    pub const TABLE: &'static str = "approval_policy_steps";
    pub const COLUMNS: &'static [&'static str] = &[
        "policy_id",
        "sequence",
        "name",
        "approver_mode",
        "role_slug",
        "department",
        "snapshot_user_key",
        "user_id",
        "condition",
        "sla_hours",
        "is_notify_only",
        "allow_self_approval",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `approval_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApprovalRequest {
    /// VARCHAR(50) NOT NULL
    pub entity_type: String,
    /// UUID NOT NULL
    pub entity_id: uuid::Uuid,
    /// UUID
    pub policy_id: Option<uuid::Uuid>,
    /// INTEGER
    pub policy_version: Option<i32>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// INTEGER NOT NULL
    pub document_revision: i32,
    /// VARCHAR(255)
    pub document_label: Option<String>,
    /// UUID
    pub submitted_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub submitted_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// INTEGER
    pub current_step_seq: Option<i32>,
    /// JSONB NOT NULL
    pub snapshot: serde_json::Value,
    /// JSONB NOT NULL
    pub flags: serde_json::Value,
    /// JSONB NOT NULL
    pub steps: serde_json::Value,
    /// TIMESTAMP WITH TIME ZONE
    pub last_reminded_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ApprovalRequest {
    pub const TABLE: &'static str = "approval_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "entity_type",
        "entity_id",
        "policy_id",
        "policy_version",
        "status",
        "document_revision",
        "document_label",
        "submitted_by_id",
        "submitted_at",
        "completed_at",
        "current_step_seq",
        "snapshot",
        "flags",
        "steps",
        "last_reminded_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `discount_allowances`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiscountAllowance {
    /// VARCHAR(30) NOT NULL
    pub kind: String,
    /// VARCHAR(255) NOT NULL
    pub label: String,
    /// NUMERIC(10, 2)
    pub min_qty: Option<rust_decimal::Decimal>,
    /// NUMERIC(10, 2)
    pub max_qty: Option<rust_decimal::Decimal>,
    /// NUMERIC(5, 2) NOT NULL
    pub max_discount_pct: rust_decimal::Decimal,
    /// BOOLEAN NOT NULL
    pub notify_owner: bool,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// VARCHAR(100)
    pub sop_reference: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl DiscountAllowance {
    pub const TABLE: &'static str = "discount_allowances";
    pub const COLUMNS: &'static [&'static str] = &[
        "kind",
        "label",
        "min_qty",
        "max_qty",
        "max_discount_pct",
        "notify_owner",
        "is_active",
        "sop_reference",
        "id",
        "created_at",
        "updated_at",
    ];
}
