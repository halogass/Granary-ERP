// Row structs of the inventory tables: one struct per table, its columns in order.

/// Row of `stock_locations`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StockLocation {
    /// UUID NOT NULL
    pub warehouse_id: uuid::Uuid,
    /// UUID
    pub parent_id: Option<uuid::Uuid>,
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// VARCHAR(30) NOT NULL
    pub code: String,
    /// VARCHAR(20)
    pub location_type: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl StockLocation {
    pub const TABLE: &'static str = "stock_locations";
    pub const COLUMNS: &'static [&'static str] = &[
        "warehouse_id",
        "parent_id",
        "name",
        "code",
        "location_type",
        "is_active",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `stock_moves`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StockMove {
    /// VARCHAR(50) NOT NULL
    pub component_sku: String,
    /// VARCHAR(255)
    pub component_name: Option<String>,
    /// UUID
    pub from_location_id: Option<uuid::Uuid>,
    /// UUID
    pub to_location_id: Option<uuid::Uuid>,
    /// NUMERIC(10, 3) NOT NULL
    pub quantity: rust_decimal::Decimal,
    /// VARCHAR(20) NOT NULL
    pub move_type: String,
    /// VARCHAR(255)
    pub reference: Option<String>,
    /// UUID
    pub moved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub moved_at: chrono::DateTime<chrono::Utc>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl StockMove {
    pub const TABLE: &'static str = "stock_moves";
    pub const COLUMNS: &'static [&'static str] = &[
        "component_sku",
        "component_name",
        "from_location_id",
        "to_location_id",
        "quantity",
        "move_type",
        "reference",
        "moved_by_id",
        "moved_at",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `warehouses`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Warehouse {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// VARCHAR(20) NOT NULL
    pub code: String,
    /// TEXT
    pub address: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_default: bool,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Warehouse {
    pub const TABLE: &'static str = "warehouses";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "code",
        "address",
        "is_default",
        "is_active",
        "id",
        "created_at",
        "updated_at",
    ];
}
