// Row structs of the manufacturing tables: one struct per table, its columns in order.

/// Row of `bom_items`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BOMItem {
    /// UUID NOT NULL
    pub bom_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub component_name: String,
    /// VARCHAR(50)
    pub component_sku: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(100)
    pub category: Option<String>,
    /// NUMERIC(10, 3) NOT NULL
    pub quantity: rust_decimal::Decimal,
    /// VARCHAR(20)
    pub unit: Option<String>,
    /// NUMERIC(18, 4)
    pub unit_cost: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub cost_currency: Option<String>,
    /// INTEGER
    pub lead_time_days: Option<i32>,
    /// VARCHAR(255)
    pub supplier_name: Option<String>,
    /// VARCHAR(100)
    pub supplier_sku: Option<String>,
    /// BOOLEAN
    pub is_critical: Option<bool>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl BOMItem {
    pub const TABLE: &'static str = "bom_items";
    pub const COLUMNS: &'static [&'static str] = &[
        "bom_id",
        "component_name",
        "component_sku",
        "description",
        "category",
        "quantity",
        "unit",
        "unit_cost",
        "cost_currency",
        "lead_time_days",
        "supplier_name",
        "supplier_sku",
        "is_critical",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `boms`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BOM {
    /// UUID NOT NULL
    pub product_id: uuid::Uuid,
    /// VARCHAR(20) NOT NULL
    pub version: String,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// TEXT
    pub notes: Option<String>,
    /// TEXT
    pub change_reason: Option<String>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
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

impl BOM {
    pub const TABLE: &'static str = "boms";
    pub const COLUMNS: &'static [&'static str] = &[
        "product_id",
        "version",
        "status",
        "is_active",
        "notes",
        "change_reason",
        "approved_by_id",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `component_inventory`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ComponentInventory {
    /// VARCHAR(50) NOT NULL
    pub component_sku: String,
    /// VARCHAR(255) NOT NULL
    pub component_name: String,
    /// VARCHAR(100)
    pub category: Option<String>,
    /// NUMERIC(10, 3)
    pub quantity_on_hand: Option<rust_decimal::Decimal>,
    /// NUMERIC(10, 3)
    pub quantity_reserved: Option<rust_decimal::Decimal>,
    /// NUMERIC(10, 3)
    pub quantity_available: Option<rust_decimal::Decimal>,
    /// VARCHAR(20)
    pub unit: Option<String>,
    /// NUMERIC(18, 4)
    pub unit_cost: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub cost_currency: Option<String>,
    /// NUMERIC(10, 3)
    pub safety_stock: Option<rust_decimal::Decimal>,
    /// NUMERIC(10, 3)
    pub reorder_point: Option<rust_decimal::Decimal>,
    /// NUMERIC(10, 3)
    pub reorder_quantity: Option<rust_decimal::Decimal>,
    /// VARCHAR(100)
    pub location: Option<String>,
    /// UUID
    pub location_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub primary_supplier: Option<String>,
    /// DATE
    pub last_purchase_date: Option<chrono::NaiveDate>,
    /// NUMERIC(18, 4)
    pub last_purchase_price: Option<rust_decimal::Decimal>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub updated_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ComponentInventory {
    pub const TABLE: &'static str = "component_inventory";
    pub const COLUMNS: &'static [&'static str] = &[
        "component_sku",
        "component_name",
        "category",
        "quantity_on_hand",
        "quantity_reserved",
        "quantity_available",
        "unit",
        "unit_cost",
        "cost_currency",
        "safety_stock",
        "reorder_point",
        "reorder_quantity",
        "location",
        "location_id",
        "primary_supplier",
        "last_purchase_date",
        "last_purchase_price",
        "notes",
        "updated_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `hardware_products`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HardwareProduct {
    /// VARCHAR(50) NOT NULL
    pub sku: String,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(50)
    pub category: Option<String>,
    /// VARCHAR(100)
    pub product_line: Option<String>,
    /// UUID
    pub product_line_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub hardware_version: Option<String>,
    /// BOOLEAN
    pub has_firmware: Option<bool>,
    /// BOOLEAN
    pub has_software: Option<bool>,
    /// NUMERIC(18, 4)
    pub unit_cost: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// UUID
    pub current_bom_id: Option<uuid::Uuid>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// VARCHAR(500)
    pub image_url: Option<String>,
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

impl HardwareProduct {
    pub const TABLE: &'static str = "hardware_products";
    pub const COLUMNS: &'static [&'static str] = &[
        "sku",
        "name",
        "description",
        "category",
        "product_line",
        "product_line_id",
        "hardware_version",
        "has_firmware",
        "has_software",
        "unit_cost",
        "currency",
        "current_bom_id",
        "is_active",
        "image_url",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `production_orders`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProductionOrder {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID NOT NULL
    pub product_id: uuid::Uuid,
    /// UUID
    pub bom_id: Option<uuid::Uuid>,
    /// VARCHAR(100)
    pub sales_order_ref: Option<String>,
    /// UUID
    pub invoice_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// INTEGER NOT NULL
    pub quantity: i32,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// BOOLEAN NOT NULL
    pub components_reserved: bool,
    /// DATE
    pub planned_start: Option<chrono::NaiveDate>,
    /// DATE
    pub planned_end: Option<chrono::NaiveDate>,
    /// DATE
    pub actual_start: Option<chrono::NaiveDate>,
    /// DATE
    pub actual_end: Option<chrono::NaiveDate>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// NUMERIC(18, 2)
    pub total_bom_cost: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub actual_material_cost: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub cost_currency: Option<String>,
    /// BOOLEAN
    pub is_subcontracted: Option<bool>,
    /// UUID
    pub subcontractor_vendor_id: Option<uuid::Uuid>,
    /// UUID
    pub subcontractor_work_order_id: Option<uuid::Uuid>,
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

impl ProductionOrder {
    pub const TABLE: &'static str = "production_orders";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "product_id",
        "bom_id",
        "sales_order_ref",
        "invoice_id",
        "customer_org_id",
        "quantity",
        "status",
        "components_reserved",
        "planned_start",
        "planned_end",
        "actual_start",
        "actual_end",
        "assigned_to_id",
        "notes",
        "created_by_id",
        "total_bom_cost",
        "actual_material_cost",
        "cost_currency",
        "is_subcontracted",
        "subcontractor_vendor_id",
        "subcontractor_work_order_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `production_units`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProductionUnit {
    /// UUID NOT NULL
    pub production_order_id: uuid::Uuid,
    /// UUID NOT NULL
    pub product_id: uuid::Uuid,
    /// UUID
    pub bom_id: Option<uuid::Uuid>,
    /// VARCHAR(100) NOT NULL
    pub serial_number: String,
    /// VARCHAR(20)
    pub hardware_revision: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub produced_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub flashed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(50)
    pub firmware_version: Option<String>,
    /// VARCHAR(50)
    pub software_version: Option<String>,
    /// UUID
    pub invoice_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub shipped_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(100)
    pub tracking_number: Option<String>,
    /// DATE
    pub delivery_date: Option<chrono::NaiveDate>,
    /// DATE
    pub warranty_expiry: Option<chrono::NaiveDate>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub produced_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ProductionUnit {
    pub const TABLE: &'static str = "production_units";
    pub const COLUMNS: &'static [&'static str] = &[
        "production_order_id",
        "product_id",
        "bom_id",
        "serial_number",
        "hardware_revision",
        "status",
        "produced_at",
        "flashed_at",
        "firmware_version",
        "software_version",
        "invoice_id",
        "shipped_at",
        "tracking_number",
        "delivery_date",
        "warranty_expiry",
        "notes",
        "produced_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `qc_records`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QCRecord {
    /// UUID NOT NULL
    pub unit_id: uuid::Uuid,
    /// UUID NOT NULL
    pub checked_by_id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub checked_at: chrono::DateTime<chrono::Utc>,
    /// VARCHAR(20) NOT NULL
    pub result: String,
    /// JSONB
    pub checklist_items: Option<serde_json::Value>,
    /// JSONB
    pub defects_found: Option<serde_json::Value>,
    /// TEXT
    pub corrective_actions: Option<String>,
    /// BOOLEAN
    pub recheck_required: Option<bool>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl QCRecord {
    pub const TABLE: &'static str = "qc_records";
    pub const COLUMNS: &'static [&'static str] = &[
        "unit_id",
        "checked_by_id",
        "checked_at",
        "result",
        "checklist_items",
        "defects_found",
        "corrective_actions",
        "recheck_required",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `rma_cases`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RMACase {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID NOT NULL
    pub warranty_id: uuid::Uuid,
    /// UUID NOT NULL
    pub unit_id: uuid::Uuid,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TEXT NOT NULL
    pub issue_description: String,
    /// VARCHAR(255)
    pub reported_by: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub reported_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub diagnosis: Option<String>,
    /// VARCHAR(50)
    pub resolution: Option<String>,
    /// TEXT
    pub resolution_notes: Option<String>,
    /// UUID
    pub replacement_unit_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub closed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub handled_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl RMACase {
    pub const TABLE: &'static str = "rma_cases";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "warranty_id",
        "unit_id",
        "customer_org_id",
        "status",
        "issue_description",
        "reported_by",
        "reported_at",
        "diagnosis",
        "resolution",
        "resolution_notes",
        "replacement_unit_id",
        "closed_at",
        "handled_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `warranty_records`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WarrantyRecord {
    /// UUID NOT NULL
    pub unit_id: uuid::Uuid,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// UUID
    pub invoice_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub customer_name: Option<String>,
    /// VARCHAR(255)
    pub customer_email: Option<String>,
    /// DATE NOT NULL
    pub delivery_date: chrono::NaiveDate,
    /// INTEGER
    pub warranty_months: Option<i32>,
    /// DATE NOT NULL
    pub expiry_date: chrono::NaiveDate,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WarrantyRecord {
    pub const TABLE: &'static str = "warranty_records";
    pub const COLUMNS: &'static [&'static str] = &[
        "unit_id",
        "customer_org_id",
        "invoice_id",
        "customer_name",
        "customer_email",
        "delivery_date",
        "warranty_months",
        "expiry_date",
        "status",
        "id",
        "created_at",
        "updated_at",
    ];
}
