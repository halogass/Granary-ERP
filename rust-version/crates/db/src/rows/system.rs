// Row structs of the system tables: one struct per table, its columns in order.

/// Row of `audit_logs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditLog {
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub user_name: Option<String>,
    /// VARCHAR(50) NOT NULL
    pub action: String,
    /// VARCHAR(100) NOT NULL
    pub entity_type: String,
    /// VARCHAR(100)
    pub entity_id: Option<String>,
    /// VARCHAR(200)
    pub entity_label: Option<String>,
    /// JSON
    pub old_values: Option<serde_json::Value>,
    /// JSON
    pub new_values: Option<serde_json::Value>,
    /// VARCHAR[]
    pub changed_fields: Option<Vec<String>>,
    /// VARCHAR(45)
    pub ip_address: Option<String>,
    /// VARCHAR(500)
    pub user_agent: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub origin: String,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AuditLog {
    pub const TABLE: &'static str = "audit_logs";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "user_name",
        "action",
        "entity_type",
        "entity_id",
        "entity_label",
        "old_values",
        "new_values",
        "changed_fields",
        "ip_address",
        "user_agent",
        "notes",
        "origin",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `document_sequences`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentSequence {
    /// VARCHAR(50) NOT NULL
    pub sequence_type: String,
    /// VARCHAR(30) NOT NULL
    pub prefix: String,
    /// VARCHAR(5)
    pub suffix_separator: Option<String>,
    /// INTEGER NOT NULL
    pub year: i32,
    /// INTEGER NOT NULL
    pub month: i32,
    /// INTEGER NOT NULL
    pub current_sequence: i32,
    /// INTEGER
    pub sequence_padding: Option<i32>,
    /// VARCHAR(100)
    pub last_generated: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl DocumentSequence {
    pub const TABLE: &'static str = "document_sequences";
    pub const COLUMNS: &'static [&'static str] = &[
        "sequence_type",
        "prefix",
        "suffix_separator",
        "year",
        "month",
        "current_sequence",
        "sequence_padding",
        "last_generated",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `notifications`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Notification {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    /// VARCHAR(100) NOT NULL
    pub type_: String,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub body: Option<String>,
    /// VARCHAR(500)
    pub link: Option<String>,
    /// VARCHAR(100)
    pub entity_type: Option<String>,
    /// VARCHAR(100)
    pub entity_id: Option<String>,
    /// BOOLEAN
    pub is_read: Option<bool>,
    /// VARCHAR(50)
    pub read_at: Option<String>,
    /// VARCHAR(20)
    pub priority: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Notification {
    pub const TABLE: &'static str = "notifications";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "type",
        "title",
        "body",
        "link",
        "entity_type",
        "entity_id",
        "is_read",
        "read_at",
        "priority",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `system_settings`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemSetting {
    /// VARCHAR(200) NOT NULL
    pub key: String,
    /// TEXT
    pub value: Option<String>,
    /// VARCHAR(20)
    pub value_type: Option<String>,
    /// VARCHAR(100) NOT NULL
    pub category: String,
    /// VARCHAR(200)
    pub label: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// BOOLEAN
    pub is_public: Option<bool>,
    /// BOOLEAN
    pub is_encrypted: Option<bool>,
    /// UUID
    pub updated_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SystemSetting {
    pub const TABLE: &'static str = "system_settings";
    pub const COLUMNS: &'static [&'static str] = &[
        "key",
        "value",
        "value_type",
        "category",
        "label",
        "description",
        "is_public",
        "is_encrypted",
        "updated_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `tax_configs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaxConfig {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// VARCHAR(3) NOT NULL
    pub country_code: String,
    /// VARCHAR(100) NOT NULL
    pub country_name: String,
    /// VARCHAR(3) NOT NULL
    pub currency_code: String,
    /// VARCHAR(10)
    pub currency_symbol: Option<String>,
    /// VARCHAR(50)
    pub currency_format: Option<String>,
    /// VARCHAR(1)
    pub decimal_separator: Option<String>,
    /// VARCHAR(1)
    pub thousands_separator: Option<String>,
    /// INTEGER
    pub fiscal_year_start_month: Option<i32>,
    /// VARCHAR(30)
    pub date_format: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// DATE NOT NULL
    pub effective_from: chrono::NaiveDate,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID
    pub updated_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TaxConfig {
    pub const TABLE: &'static str = "tax_configs";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "country_code",
        "country_name",
        "currency_code",
        "currency_symbol",
        "currency_format",
        "decimal_separator",
        "thousands_separator",
        "fiscal_year_start_month",
        "date_format",
        "is_active",
        "effective_from",
        "notes",
        "created_by_id",
        "updated_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `tax_rules`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaxRule {
    /// UUID NOT NULL
    pub config_id: uuid::Uuid,
    /// VARCHAR(50) NOT NULL
    pub code: String,
    /// VARCHAR(150) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// NUMERIC(8, 4) NOT NULL
    pub rate: rust_decimal::Decimal,
    /// BOOLEAN
    pub is_percentage: Option<bool>,
    /// NUMERIC(18, 2)
    pub fixed_amount: Option<rust_decimal::Decimal>,
    /// BOOLEAN
    pub is_compound: Option<bool>,
    /// VARCHAR[]
    pub compound_base_codes: Option<Vec<String>>,
    /// VARCHAR(50) NOT NULL
    pub tax_type: String,
    /// BOOLEAN
    pub is_withholding: Option<bool>,
    /// VARCHAR(20)
    pub withheld_by: Option<String>,
    /// BOOLEAN
    pub applies_to_sales: Option<bool>,
    /// BOOLEAN
    pub applies_to_purchases: Option<bool>,
    /// BOOLEAN
    pub applies_to_services: Option<bool>,
    /// BOOLEAN
    pub applies_to_products: Option<bool>,
    /// BOOLEAN
    pub applies_to_imports: Option<bool>,
    /// BOOLEAN
    pub applies_to_vendor_invoices: Option<bool>,
    /// BOOLEAN
    pub show_on_quotation: Option<bool>,
    /// BOOLEAN
    pub show_on_invoice: Option<bool>,
    /// BOOLEAN
    pub show_on_vendor_invoice: Option<bool>,
    /// NUMERIC(18, 2)
    pub minimum_taxable_amount: Option<rust_decimal::Decimal>,
    /// BOOLEAN
    pub registration_number_required: Option<bool>,
    /// VARCHAR(50)
    pub tax_account_code: Option<String>,
    /// VARCHAR(50)
    pub reporting_code: Option<String>,
    /// INTEGER
    pub display_order: Option<i32>,
    /// BOOLEAN
    pub is_active: Option<bool>,
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

impl TaxRule {
    pub const TABLE: &'static str = "tax_rules";
    pub const COLUMNS: &'static [&'static str] = &[
        "config_id",
        "code",
        "name",
        "description",
        "rate",
        "is_percentage",
        "fixed_amount",
        "is_compound",
        "compound_base_codes",
        "tax_type",
        "is_withholding",
        "withheld_by",
        "applies_to_sales",
        "applies_to_purchases",
        "applies_to_services",
        "applies_to_products",
        "applies_to_imports",
        "applies_to_vendor_invoices",
        "show_on_quotation",
        "show_on_invoice",
        "show_on_vendor_invoice",
        "minimum_taxable_amount",
        "registration_number_required",
        "tax_account_code",
        "reporting_code",
        "display_order",
        "is_active",
        "notes",
        "updated_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `whatsapp_sessions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WhatsAppSession {
    /// VARCHAR(50) NOT NULL
    pub phone_number: String,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TEXT
    pub qr_code_data: Option<String>,
    /// VARCHAR(50)
    pub qr_expires_at: Option<String>,
    /// VARCHAR(50)
    pub last_active_at: Option<String>,
    /// JSON
    pub session_data: Option<serde_json::Value>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WhatsAppSession {
    pub const TABLE: &'static str = "whatsapp_sessions";
    pub const COLUMNS: &'static [&'static str] = &[
        "phone_number",
        "status",
        "qr_code_data",
        "qr_expires_at",
        "last_active_at",
        "session_data",
        "id",
        "created_at",
        "updated_at",
    ];
}
