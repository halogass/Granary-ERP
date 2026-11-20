// Row structs of the p2p tables: one struct per table, its columns in order.

/// Row of `purchase_rfq_line_responses`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PurchaseRFQLineResponse {
    /// UUID NOT NULL
    pub rfq_id: uuid::Uuid,
    /// UUID NOT NULL
    pub rfq_line_id: uuid::Uuid,
    /// UUID NOT NULL
    pub vendor_id: uuid::Uuid,
    /// NUMERIC(18, 4) NOT NULL
    pub unit_price: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(5, 0)
    pub lead_time_days: Option<rust_decimal::Decimal>,
    /// TEXT
    pub notes: Option<String>,
    /// BOOLEAN
    pub is_selected: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PurchaseRFQLineResponse {
    pub const TABLE: &'static str = "purchase_rfq_line_responses";
    pub const COLUMNS: &'static [&'static str] = &[
        "rfq_id",
        "rfq_line_id",
        "vendor_id",
        "unit_price",
        "currency",
        "lead_time_days",
        "notes",
        "is_selected",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `purchase_rfq_lines`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PurchaseRFQLine {
    /// UUID NOT NULL
    pub rfq_id: uuid::Uuid,
    /// NUMERIC(5, 0)
    pub sort_order: Option<rust_decimal::Decimal>,
    /// VARCHAR(50)
    pub component_sku: Option<String>,
    /// VARCHAR(500) NOT NULL
    pub description: String,
    /// NUMERIC(10, 3) NOT NULL
    pub quantity: rust_decimal::Decimal,
    /// VARCHAR(20)
    pub unit: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PurchaseRFQLine {
    pub const TABLE: &'static str = "purchase_rfq_lines";
    pub const COLUMNS: &'static [&'static str] = &[
        "rfq_id",
        "sort_order",
        "component_sku",
        "description",
        "quantity",
        "unit",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `purchase_rfq_vendor_invites`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PurchaseRFQVendorInvite {
    /// UUID NOT NULL
    pub rfq_id: uuid::Uuid,
    /// UUID NOT NULL
    pub vendor_id: uuid::Uuid,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub invited_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub responded_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub response_notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PurchaseRFQVendorInvite {
    pub const TABLE: &'static str = "purchase_rfq_vendor_invites";
    pub const COLUMNS: &'static [&'static str] = &[
        "rfq_id",
        "vendor_id",
        "status",
        "invited_at",
        "responded_at",
        "response_notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `purchase_rfqs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PurchaseRFQ {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// DATE
    pub deadline: Option<chrono::NaiveDate>,
    /// VARCHAR(3)
    pub currency: Option<String>,
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

impl PurchaseRFQ {
    pub const TABLE: &'static str = "purchase_rfqs";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "title",
        "description",
        "status",
        "deadline",
        "currency",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
