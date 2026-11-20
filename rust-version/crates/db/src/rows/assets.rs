// Row structs of the assets tables: one struct per table, its columns in order.

/// Row of `asset_categories`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssetCategory {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(30)
    pub depreciation_method: Option<String>,
    /// INTEGER
    pub useful_life_months: Option<i32>,
    /// NUMERIC(5, 2)
    pub salvage_value_pct: Option<rust_decimal::Decimal>,
    /// UUID
    pub gl_asset_account_id: Option<uuid::Uuid>,
    /// UUID
    pub gl_depreciation_account_id: Option<uuid::Uuid>,
    /// UUID
    pub gl_accumulated_account_id: Option<uuid::Uuid>,
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

impl AssetCategory {
    pub const TABLE: &'static str = "asset_categories";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "description",
        "depreciation_method",
        "useful_life_months",
        "salvage_value_pct",
        "gl_asset_account_id",
        "gl_depreciation_account_id",
        "gl_accumulated_account_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `asset_depreciation_lines`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssetDepreciationLine {
    /// UUID NOT NULL
    pub asset_id: uuid::Uuid,
    /// DATE NOT NULL
    pub period_date: chrono::NaiveDate,
    /// VARCHAR(20)
    pub period_label: Option<String>,
    /// NUMERIC(18, 2) NOT NULL
    pub depreciation_amount: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub accumulated_depreciation: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub book_value: rust_decimal::Decimal,
    /// UUID
    pub journal_entry_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AssetDepreciationLine {
    pub const TABLE: &'static str = "asset_depreciation_lines";
    pub const COLUMNS: &'static [&'static str] = &[
        "asset_id",
        "period_date",
        "period_label",
        "depreciation_amount",
        "accumulated_depreciation",
        "book_value",
        "journal_entry_id",
        "status",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `asset_disposals`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssetDisposal {
    /// UUID NOT NULL
    pub asset_id: uuid::Uuid,
    /// DATE NOT NULL
    pub disposal_date: chrono::NaiveDate,
    /// VARCHAR(30) NOT NULL
    pub disposal_type: String,
    /// NUMERIC(18, 2)
    pub proceeds: Option<rust_decimal::Decimal>,
    /// UUID
    pub bank_account_id: Option<uuid::Uuid>,
    /// TEXT
    pub disposal_notes: Option<String>,
    /// UUID
    pub journal_entry_id: Option<uuid::Uuid>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AssetDisposal {
    pub const TABLE: &'static str = "asset_disposals";
    pub const COLUMNS: &'static [&'static str] = &[
        "asset_id",
        "disposal_date",
        "disposal_type",
        "proceeds",
        "bank_account_id",
        "disposal_notes",
        "journal_entry_id",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `fixed_assets`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FixedAsset {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// UUID
    pub category_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// DATE NOT NULL
    pub purchase_date: chrono::NaiveDate,
    /// NUMERIC(18, 2) NOT NULL
    pub purchase_price: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(24, 12)
    pub acquisition_rate: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub useful_life_months: Option<i32>,
    /// NUMERIC(18, 2)
    pub salvage_value: Option<rust_decimal::Decimal>,
    /// VARCHAR(30)
    pub depreciation_method: Option<String>,
    /// VARCHAR(200)
    pub location: Option<String>,
    /// VARCHAR(100)
    pub serial_number: Option<String>,
    /// UUID
    pub vendor_id: Option<uuid::Uuid>,
    /// UUID
    pub purchase_order_id: Option<uuid::Uuid>,
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

impl FixedAsset {
    pub const TABLE: &'static str = "fixed_assets";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "name",
        "description",
        "category_id",
        "status",
        "purchase_date",
        "purchase_price",
        "currency",
        "acquisition_rate",
        "useful_life_months",
        "salvage_value",
        "depreciation_method",
        "location",
        "serial_number",
        "vendor_id",
        "purchase_order_id",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
