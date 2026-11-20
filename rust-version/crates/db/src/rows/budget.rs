// Row structs of the budget tables: one struct per table, its columns in order.

/// Row of `budget_lines`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BudgetLine {
    /// UUID NOT NULL
    pub budget_id: uuid::Uuid,
    /// UUID NOT NULL
    pub account_id: uuid::Uuid,
    /// VARCHAR(255)
    pub label: Option<String>,
    /// NUMERIC(18, 2) NOT NULL
    pub budgeted_amount: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// INTEGER
    pub period_month: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl BudgetLine {
    pub const TABLE: &'static str = "budget_lines";
    pub const COLUMNS: &'static [&'static str] = &[
        "budget_id",
        "account_id",
        "label",
        "budgeted_amount",
        "currency",
        "period_month",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `budgets`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Budget {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// INTEGER NOT NULL
    pub fiscal_year: i32,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// UUID
    pub project_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub period_type: Option<String>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub notes: Option<String>,
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

impl Budget {
    pub const TABLE: &'static str = "budgets";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "fiscal_year",
        "department",
        "project_id",
        "period_type",
        "status",
        "approved_by_id",
        "approved_at",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
