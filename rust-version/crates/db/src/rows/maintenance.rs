// Row structs of the maintenance tables: one struct per table, its columns in order.

/// Row of `maintenance_equipment`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaintenanceEquipment {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(50) NOT NULL
    pub code: String,
    /// VARCHAR(100)
    pub equipment_type: Option<String>,
    /// VARCHAR(100)
    pub brand: Option<String>,
    /// VARCHAR(100)
    pub model: Option<String>,
    /// VARCHAR(100)
    pub serial_number: Option<String>,
    /// VARCHAR(200)
    pub location: Option<String>,
    /// DATE
    pub purchase_date: Option<chrono::NaiveDate>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub asset_id: Option<uuid::Uuid>,
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

impl MaintenanceEquipment {
    pub const TABLE: &'static str = "maintenance_equipment";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "code",
        "equipment_type",
        "brand",
        "model",
        "serial_number",
        "location",
        "purchase_date",
        "status",
        "asset_id",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `maintenance_plans`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaintenancePlan {
    /// UUID NOT NULL
    pub equipment_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub frequency_type: String,
    /// INTEGER NOT NULL
    pub frequency_value: i32,
    /// TIMESTAMP WITH TIME ZONE
    pub last_performed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub next_due_at: Option<chrono::DateTime<chrono::Utc>>,
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
    /// BOOLEAN NOT NULL
    pub is_deleted: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl MaintenancePlan {
    pub const TABLE: &'static str = "maintenance_plans";
    pub const COLUMNS: &'static [&'static str] = &[
        "equipment_id",
        "name",
        "description",
        "frequency_type",
        "frequency_value",
        "last_performed_at",
        "next_due_at",
        "is_active",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `maintenance_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaintenanceRequest {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID NOT NULL
    pub equipment_id: uuid::Uuid,
    /// UUID
    pub reported_by_id: Option<uuid::Uuid>,
    /// TEXT NOT NULL
    pub description: String,
    /// VARCHAR(20)
    pub priority: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub submitted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub reviewed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub rejection_reason: Option<String>,
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

impl MaintenanceRequest {
    pub const TABLE: &'static str = "maintenance_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "equipment_id",
        "reported_by_id",
        "description",
        "priority",
        "status",
        "submitted_at",
        "reviewed_by_id",
        "reviewed_at",
        "rejection_reason",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `maintenance_work_orders`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaintenanceWorkOrder {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID
    pub request_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub equipment_id: uuid::Uuid,
    /// VARCHAR(20)
    pub work_type: Option<String>,
    /// TEXT NOT NULL
    pub description: String,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// DATE
    pub scheduled_date: Option<chrono::NaiveDate>,
    /// TIMESTAMP WITH TIME ZONE
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// NUMERIC(6, 2)
    pub downtime_hours: Option<rust_decimal::Decimal>,
    /// NUMERIC(14, 2)
    pub cost: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// UUID
    pub vendor_id: Option<uuid::Uuid>,
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

impl MaintenanceWorkOrder {
    pub const TABLE: &'static str = "maintenance_work_orders";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "request_id",
        "equipment_id",
        "work_type",
        "description",
        "status",
        "assigned_to_id",
        "scheduled_date",
        "started_at",
        "completed_at",
        "downtime_hours",
        "cost",
        "currency",
        "vendor_id",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
