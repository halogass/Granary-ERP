// Row structs of the sales tables: one struct per table, its columns in order.

/// Row of `catalog_prices`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CatalogPrice {
    /// UUID NOT NULL
    pub product_id: uuid::Uuid,
    /// VARCHAR(3) NOT NULL
    pub currency: String,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// DATE NOT NULL
    pub valid_from: chrono::NaiveDate,
    /// DATE
    pub valid_to: Option<chrono::NaiveDate>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CatalogPrice {
    pub const TABLE: &'static str = "catalog_prices";
    pub const COLUMNS: &'static [&'static str] = &[
        "product_id",
        "currency",
        "amount",
        "valid_from",
        "valid_to",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `catalog_volume_tiers`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CatalogVolumeTier {
    /// UUID NOT NULL
    pub product_id: uuid::Uuid,
    /// NUMERIC(10, 2) NOT NULL
    pub min_qty: rust_decimal::Decimal,
    /// NUMERIC(10, 2)
    pub max_qty: Option<rust_decimal::Decimal>,
    /// NUMERIC(6, 3) NOT NULL
    pub discount_pct: rust_decimal::Decimal,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CatalogVolumeTier {
    pub const TABLE: &'static str = "catalog_volume_tiers";
    pub const COLUMNS: &'static [&'static str] = &[
        "product_id",
        "min_qty",
        "max_qty",
        "discount_pct",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `contracts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Contract {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID
    pub quotation_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_contact_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(50)
    pub contract_type: Option<String>,
    /// NUMERIC(18, 2)
    pub value: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// BOOLEAN
    pub auto_renew: Option<bool>,
    /// TIMESTAMP WITH TIME ZONE
    pub signed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(255)
    pub signed_by_customer_name: Option<String>,
    /// VARCHAR(255)
    pub signed_by_customer_title: Option<String>,
    /// UUID
    pub signed_by_company_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub signed_by_company_name: Option<String>,
    /// VARCHAR(500)
    pub document_url: Option<String>,
    /// VARCHAR(100)
    pub document_id: Option<String>,
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

impl Contract {
    pub const TABLE: &'static str = "contracts";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "quotation_id",
        "deal_id",
        "customer_org_id",
        "customer_contact_id",
        "status",
        "contract_type",
        "value",
        "currency",
        "start_date",
        "end_date",
        "auto_renew",
        "signed_at",
        "signed_by_customer_name",
        "signed_by_customer_title",
        "signed_by_company_id",
        "signed_by_company_name",
        "document_url",
        "document_id",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `note_templates`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoteTemplate {
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// TEXT NOT NULL
    pub body: String,
    /// VARCHAR(30)
    pub doc_type: Option<String>,
    /// INTEGER
    pub sort_order: Option<i32>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NoteTemplate {
    pub const TABLE: &'static str = "note_templates";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "body",
        "doc_type",
        "sort_order",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `product_catalog`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProductCatalog {
    /// VARCHAR(50) NOT NULL
    pub sku: String,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(100)
    pub short_name: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(100)
    pub category: Option<String>,
    /// VARCHAR(50)
    pub family: Option<String>,
    /// VARCHAR(30) NOT NULL
    pub product_type: String,
    /// VARCHAR(30)
    pub unit: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub sale_model: String,
    /// VARCHAR(20) NOT NULL
    pub price_metric: String,
    /// INTEGER
    pub rate_period_months: Option<i32>,
    /// INTEGER
    pub billing_interval_months: Option<i32>,
    /// NUMERIC(10, 2)
    pub min_quantity: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub min_term_months: Option<i32>,
    /// NUMERIC(6, 2)
    pub unit_equivalents: Option<rust_decimal::Decimal>,
    /// VARCHAR[]
    pub modules: Option<Vec<String>>,
    /// INTEGER
    pub included_support_months: Option<i32>,
    /// VARCHAR(20) NOT NULL
    pub pricing_mode: String,
    /// NUMERIC(6, 3)
    pub percent_rate: Option<rust_decimal::Decimal>,
    /// VARCHAR[]
    pub addon_for: Option<Vec<String>>,
    /// INTEGER
    pub whitelabel_free_min_qty: Option<i32>,
    /// JSONB
    pub whitelabel_rule: Option<serde_json::Value>,
    /// BOOLEAN NOT NULL
    pub is_whitelabel: bool,
    /// TEXT
    pub terms_note: Option<String>,
    /// NUMERIC(18, 2)
    pub cost_price: Option<rust_decimal::Decimal>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// BOOLEAN
    pub is_orderable: Option<bool>,
    /// VARCHAR(500)
    pub image_url: Option<String>,
    /// UUID
    pub hardware_product_id: Option<uuid::Uuid>,
    /// UUID
    pub product_line_id: Option<uuid::Uuid>,
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
    /// VARCHAR(30): the segment (services::segment)
    #[serde(default)]
    pub segment: Option<String>,
    /// VARCHAR(10) NOT NULL DEFAULT 'locked' (migration 0159): locked | proposed
    #[serde(default)]
    pub price_status: Option<String>,
}

impl ProductCatalog {
    pub const TABLE: &'static str = "product_catalog";
    pub const COLUMNS: &'static [&'static str] = &[
        "sku",
        "name",
        "short_name",
        "description",
        "category",
        "family",
        "product_type",
        "unit",
        "sale_model",
        "price_metric",
        "rate_period_months",
        "billing_interval_months",
        "min_quantity",
        "min_term_months",
        "unit_equivalents",
        "modules",
        "included_support_months",
        "pricing_mode",
        "percent_rate",
        "addon_for",
        "whitelabel_free_min_qty",
        "whitelabel_rule",
        "is_whitelabel",
        "terms_note",
        "cost_price",
        "is_active",
        "is_orderable",
        "image_url",
        "hardware_product_id",
        "product_line_id",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
        "segment",
        "price_status",
    ];
}

/// Row of `quotation_items`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuotationItem {
    /// UUID NOT NULL
    pub quotation_id: uuid::Uuid,
    /// UUID
    pub product_id: Option<uuid::Uuid>,
    /// INTEGER
    pub sort_order: Option<i32>,
    /// VARCHAR(50)
    pub sku: Option<String>,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
    /// NUMERIC(10, 2) NOT NULL
    pub quantity: rust_decimal::Decimal,
    /// VARCHAR(30)
    pub unit: Option<String>,
    /// NUMERIC(18, 2) NOT NULL
    pub unit_price: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(5, 2)
    pub discount_percentage: Option<rust_decimal::Decimal>,
    /// VARCHAR(20)
    pub discount_type: Option<String>,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// VARCHAR(500)
    pub image_url: Option<String>,
    /// BOOLEAN
    pub is_optional: Option<bool>,
    /// UUID
    pub parent_item_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub sale_model: Option<String>,
    /// VARCHAR(20)
    pub price_metric: Option<String>,
    /// INTEGER
    pub rate_period_months: Option<i32>,
    /// INTEGER
    pub billing_interval_months: Option<i32>,
    /// INTEGER
    pub term_months: Option<i32>,
    /// NUMERIC(18, 2)
    pub list_unit_price: Option<rust_decimal::Decimal>,
    /// VARCHAR(30)
    pub price_rule: Option<String>,
    /// UUID
    pub tax_id: Option<uuid::Uuid>,
    /// VARCHAR(50)
    pub tax_code: Option<String>,
    /// NUMERIC(8, 4) NOT NULL
    pub tax_pct: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub tax_amount: rust_decimal::Decimal,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl QuotationItem {
    pub const TABLE: &'static str = "quotation_items";
    pub const COLUMNS: &'static [&'static str] = &[
        "quotation_id",
        "product_id",
        "sort_order",
        "sku",
        "name",
        "description",
        "quantity",
        "unit",
        "unit_price",
        "currency",
        "discount_percentage",
        "discount_type",
        "amount",
        "image_url",
        "is_optional",
        "parent_item_id",
        "sale_model",
        "price_metric",
        "rate_period_months",
        "billing_interval_months",
        "term_months",
        "list_unit_price",
        "price_rule",
        "tax_id",
        "tax_code",
        "tax_pct",
        "tax_amount",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `quotations`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Quotation {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// INTEGER
    pub revision: Option<i32>,
    /// VARCHAR(80)
    pub full_number: Option<String>,
    /// VARCHAR(50)
    pub revision_group: Option<String>,
    /// UUID
    pub previous_version_id: Option<uuid::Uuid>,
    /// TEXT
    pub revision_reason: Option<String>,
    /// VARCHAR(500)
    pub subject: Option<String>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_contact_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(24, 12)
    pub exchange_rate: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub subtotal: Option<rust_decimal::Decimal>,
    /// VARCHAR(20)
    pub discount_type: Option<String>,
    /// NUMERIC(18, 2)
    pub discount_value: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub discount_amount: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub tax_amount: Option<rust_decimal::Decimal>,
    /// JSONB
    pub tax_lines: Option<serde_json::Value>,
    /// NUMERIC(18, 2)
    pub total: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub rounded_total: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub total_quantity: Option<i32>,
    /// DATE NOT NULL
    pub issue_date: chrono::NaiveDate,
    /// DATE NOT NULL
    pub valid_until: chrono::NaiveDate,
    /// INTEGER
    pub validity_days: Option<i32>,
    /// JSONB
    pub payment_terms: Option<serde_json::Value>,
    /// INTEGER
    pub payment_terms_days: Option<i32>,
    /// VARCHAR(20)
    pub procurement_type: Option<String>,
    /// VARCHAR(50)
    pub shipping_terms: Option<String>,
    /// INTEGER
    pub lead_time_days_min: Option<i32>,
    /// INTEGER
    pub lead_time_days_max: Option<i32>,
    /// VARCHAR(100)
    pub customer_po_number: Option<String>,
    /// DATE
    pub customer_po_date: Option<chrono::NaiveDate>,
    /// BOOLEAN
    pub customer_po_accepted_verbally: Option<bool>,
    /// TEXT
    pub notes: Option<String>,
    /// TEXT
    pub internal_notes: Option<String>,
    /// VARCHAR(500)
    pub pdf_url: Option<String>,
    /// VARCHAR(100)
    pub document_id: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub signed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(255)
    pub signed_by_customer: Option<String>,
    /// JSONB
    pub signature_data: Option<serde_json::Value>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
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
    /// TEXT (migration 0144): why the header discount was given, printed under the Discount line or in the notes
    #[serde(default)]
    pub discount_note: Option<String>,
    /// VARCHAR(20) (migration 0144): 'pdf' (a Discount line) or 'notes_only' (totals stay full, the note says it)
    #[serde(default)]
    pub discount_display: Option<String>,
    /// TIMESTAMP WITH TIME ZONE (migration 0144)
    #[serde(default)]
    pub cancelled_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT (migration 0144)
    #[serde(default)]
    pub cancel_reason: Option<String>,
    /// TEXT (migration 0159): the Owner's reason to let the quotation through the pricing rules
    #[serde(default)]
    pub pricing_override_reason: Option<String>,
    /// UUID (migration 0159)
    #[serde(default)]
    pub pricing_override_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE (migration 0159)
    #[serde(default)]
    pub pricing_override_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(64) (migration 0159): the lines the override was given for
    #[serde(default)]
    pub pricing_override_fingerprint: Option<String>,
}

impl Quotation {
    pub const TABLE: &'static str = "quotations";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "revision",
        "full_number",
        "revision_group",
        "previous_version_id",
        "revision_reason",
        "subject",
        "deal_id",
        "customer_org_id",
        "customer_contact_id",
        "status",
        "currency",
        "exchange_rate",
        "subtotal",
        "discount_type",
        "discount_value",
        "discount_amount",
        "tax_amount",
        "tax_lines",
        "total",
        "rounded_total",
        "total_quantity",
        "issue_date",
        "valid_until",
        "validity_days",
        "payment_terms",
        "payment_terms_days",
        "procurement_type",
        "shipping_terms",
        "lead_time_days_min",
        "lead_time_days_max",
        "customer_po_number",
        "customer_po_date",
        "customer_po_accepted_verbally",
        "notes",
        "internal_notes",
        "pdf_url",
        "document_id",
        "signed_at",
        "signed_by_customer",
        "signature_data",
        "created_by_id",
        "approved_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
        "discount_note",
        "discount_display",
        "cancelled_at",
        "cancel_reason",
        "pricing_override_reason",
        "pricing_override_by_id",
        "pricing_override_at",
        "pricing_override_fingerprint",
    ];
}
