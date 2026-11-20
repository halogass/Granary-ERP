// Row structs of the product_line tables: one struct per table, its columns in order.

/// Row of `product_lines`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProductLine {
    /// VARCHAR(64) NOT NULL
    pub code: String,
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// VARCHAR(30)
    pub category: Option<String>,
    /// VARCHAR(16)
    pub color: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// INTEGER
    pub sort_order: Option<i32>,
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

impl ProductLine {
    pub const TABLE: &'static str = "product_lines";
    pub const COLUMNS: &'static [&'static str] = &[
        "code",
        "name",
        "category",
        "color",
        "description",
        "is_active",
        "sort_order",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
