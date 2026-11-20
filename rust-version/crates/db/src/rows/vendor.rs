// Row structs of the vendor tables: one struct per table, its columns in order.

/// Row of `contractor_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContractorRequest {
    /// UUID NOT NULL
    pub requested_by_id: uuid::Uuid,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// VARCHAR(50)
    pub vendor_type: Option<String>,
    /// TEXT NOT NULL
    pub scope_description: String,
    /// NUMERIC(18, 2)
    pub estimated_value: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// DATE
    pub needed_by: Option<chrono::NaiveDate>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub rejection_reason: Option<String>,
    /// UUID
    pub preferred_vendor_id: Option<uuid::Uuid>,
    /// UUID
    pub selected_vendor_id: Option<uuid::Uuid>,
    /// UUID
    pub work_order_id: Option<uuid::Uuid>,
    /// UUID
    pub purchase_order_id: Option<uuid::Uuid>,
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

impl ContractorRequest {
    pub const TABLE: &'static str = "contractor_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "requested_by_id",
        "department",
        "vendor_type",
        "scope_description",
        "estimated_value",
        "currency",
        "needed_by",
        "status",
        "approved_by_id",
        "approved_at",
        "rejection_reason",
        "preferred_vendor_id",
        "selected_vendor_id",
        "work_order_id",
        "purchase_order_id",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `goods_receipts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoodsReceipt {
    /// VARCHAR(50)
    pub receipt_number: Option<String>,
    /// UUID NOT NULL
    pub po_id: uuid::Uuid,
    /// UUID NOT NULL
    pub received_by_id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub received_at: chrono::DateTime<chrono::Utc>,
    /// DATE
    pub received_date: Option<chrono::NaiveDate>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// UUID
    pub location_id: Option<uuid::Uuid>,
    /// JSONB
    pub items_received: Option<serde_json::Value>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl GoodsReceipt {
    pub const TABLE: &'static str = "goods_receipts";
    pub const COLUMNS: &'static [&'static str] = &[
        "receipt_number",
        "po_id",
        "received_by_id",
        "received_at",
        "received_date",
        "status",
        "location_id",
        "items_received",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `purchase_order_items`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PurchaseOrderItem {
    /// UUID NOT NULL
    pub po_id: uuid::Uuid,
    /// VARCHAR(50)
    pub component_sku: Option<String>,
    /// VARCHAR(500) NOT NULL
    pub description: String,
    /// NUMERIC(10, 3) NOT NULL
    pub quantity_ordered: rust_decimal::Decimal,
    /// NUMERIC(10, 3)
    pub quantity_received: Option<rust_decimal::Decimal>,
    /// NUMERIC(10, 3)
    pub quantity_invoiced: Option<rust_decimal::Decimal>,
    /// VARCHAR(20)
    pub unit: Option<String>,
    /// NUMERIC(18, 4) NOT NULL
    pub unit_price: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// UUID
    pub rfq_line_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PurchaseOrderItem {
    pub const TABLE: &'static str = "purchase_order_items";
    pub const COLUMNS: &'static [&'static str] = &[
        "po_id",
        "component_sku",
        "description",
        "quantity_ordered",
        "quantity_received",
        "quantity_invoiced",
        "unit",
        "unit_price",
        "currency",
        "amount",
        "rfq_line_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `purchase_orders`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PurchaseOrder {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID NOT NULL
    pub vendor_id: uuid::Uuid,
    /// UUID
    pub contractor_request_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 2)
    pub subtotal: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub tax_amount: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub shipping_cost: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub total: Option<rust_decimal::Decimal>,
    /// DATE
    pub order_date: Option<chrono::NaiveDate>,
    /// DATE
    pub expected_delivery: Option<chrono::NaiveDate>,
    /// DATE
    pub actual_delivery: Option<chrono::NaiveDate>,
    /// TEXT
    pub shipping_address: Option<String>,
    /// VARCHAR(20)
    pub incoterms: Option<String>,
    /// UUID
    pub rfq_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub approval_status: Option<String>,
    /// UUID NOT NULL
    pub requested_by_id: uuid::Uuid,
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

impl PurchaseOrder {
    pub const TABLE: &'static str = "purchase_orders";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "vendor_id",
        "contractor_request_id",
        "status",
        "currency",
        "subtotal",
        "tax_amount",
        "shipping_cost",
        "total",
        "order_date",
        "expected_delivery",
        "actual_delivery",
        "shipping_address",
        "incoterms",
        "rfq_id",
        "approval_status",
        "requested_by_id",
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

/// Row of `vendor_documents`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VendorDocument {
    /// UUID NOT NULL
    pub vendor_id: uuid::Uuid,
    /// VARCHAR(50) NOT NULL
    pub document_type: String,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(500)
    pub file_url: Option<String>,
    /// DATE
    pub signed_at: Option<chrono::NaiveDate>,
    /// DATE
    pub expires_at: Option<chrono::NaiveDate>,
    /// BOOLEAN
    pub is_verified: Option<bool>,
    /// UUID
    pub verified_by_id: Option<uuid::Uuid>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl VendorDocument {
    pub const TABLE: &'static str = "vendor_documents";
    pub const COLUMNS: &'static [&'static str] = &[
        "vendor_id",
        "document_type",
        "name",
        "file_url",
        "signed_at",
        "expires_at",
        "is_verified",
        "verified_by_id",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `vendor_invoices`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VendorInvoice {
    /// UUID
    pub work_order_id: Option<uuid::Uuid>,
    /// UUID
    pub purchase_order_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub vendor_id: uuid::Uuid,
    /// VARCHAR(100)
    pub vendor_invoice_number: Option<String>,
    /// DATE NOT NULL
    pub invoice_date: chrono::NaiveDate,
    /// DATE
    pub due_date: Option<chrono::NaiveDate>,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// NUMERIC(18, 2)
    pub tax_amount: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub withholding_amount: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub net_payable: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(500)
    pub invoice_file_url: Option<String>,
    /// TEXT
    pub rejection_reason: Option<String>,
    /// UUID
    pub reviewed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub paid_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(255)
    pub payment_reference: Option<String>,
    /// NUMERIC(24, 12)
    pub exchange_rate: Option<rust_decimal::Decimal>,
    /// UUID
    pub bank_account_id: Option<uuid::Uuid>,
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

impl VendorInvoice {
    pub const TABLE: &'static str = "vendor_invoices";
    pub const COLUMNS: &'static [&'static str] = &[
        "work_order_id",
        "purchase_order_id",
        "vendor_id",
        "vendor_invoice_number",
        "invoice_date",
        "due_date",
        "amount",
        "tax_amount",
        "withholding_amount",
        "net_payable",
        "currency",
        "status",
        "invoice_file_url",
        "rejection_reason",
        "reviewed_by_id",
        "reviewed_at",
        "paid_at",
        "payment_reference",
        "exchange_rate",
        "bank_account_id",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `vendors`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Vendor {
    /// UUID
    pub organization_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(50) NOT NULL
    pub vendor_type: String,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(50)
    pub tax_id: Option<String>,
    /// VARCHAR(100)
    pub registration_number: Option<String>,
    /// VARCHAR(100)
    pub bank_name: Option<String>,
    /// VARCHAR(255)
    pub bank_account_name: Option<String>,
    /// VARCHAR(50)
    pub bank_account_number: Option<String>,
    /// VARCHAR(20)
    pub bank_swift: Option<String>,
    /// VARCHAR(100)
    pub bank_country: Option<String>,
    /// BOOLEAN
    pub withholding_applicable: Option<bool>,
    /// NUMERIC(6, 3)
    pub withholding_rate: Option<rust_decimal::Decimal>,
    /// VARCHAR(255)
    pub contact_name: Option<String>,
    /// VARCHAR(255)
    pub contact_email: Option<String>,
    /// VARCHAR(50)
    pub contact_phone: Option<String>,
    /// VARCHAR(50)
    pub contact_whatsapp: Option<String>,
    /// TEXT
    pub address: Option<String>,
    /// VARCHAR(100)
    pub country: Option<String>,
    /// BOOLEAN
    pub has_portal_access: Option<bool>,
    /// NUMERIC(3, 2)
    pub rating: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub total_spend: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub spend_currency: Option<String>,
    /// DATE
    pub onboarded_at: Option<chrono::NaiveDate>,
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

impl Vendor {
    pub const TABLE: &'static str = "vendors";
    pub const COLUMNS: &'static [&'static str] = &[
        "organization_id",
        "name",
        "vendor_type",
        "status",
        "tax_id",
        "registration_number",
        "bank_name",
        "bank_account_name",
        "bank_account_number",
        "bank_swift",
        "bank_country",
        "withholding_applicable",
        "withholding_rate",
        "contact_name",
        "contact_email",
        "contact_phone",
        "contact_whatsapp",
        "address",
        "country",
        "has_portal_access",
        "rating",
        "total_spend",
        "spend_currency",
        "onboarded_at",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `work_order_timesheets`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkOrderTimesheet {
    /// UUID NOT NULL
    pub work_order_id: uuid::Uuid,
    /// UUID
    pub vendor_user_id: Option<uuid::Uuid>,
    /// DATE NOT NULL
    pub period_start: chrono::NaiveDate,
    /// DATE NOT NULL
    pub period_end: chrono::NaiveDate,
    /// NUMERIC(6, 2) NOT NULL
    pub total_hours: rust_decimal::Decimal,
    /// JSONB
    pub daily_breakdown: Option<serde_json::Value>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub reviewed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub rejection_reason: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WorkOrderTimesheet {
    pub const TABLE: &'static str = "work_order_timesheets";
    pub const COLUMNS: &'static [&'static str] = &[
        "work_order_id",
        "vendor_user_id",
        "period_start",
        "period_end",
        "total_hours",
        "daily_breakdown",
        "status",
        "reviewed_by_id",
        "reviewed_at",
        "rejection_reason",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `work_orders`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkOrder {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID NOT NULL
    pub vendor_id: uuid::Uuid,
    /// UUID
    pub contractor_request_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// TEXT
    pub scope: Option<String>,
    /// VARCHAR(50)
    pub work_type: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub payment_type: String,
    /// NUMERIC(18, 2)
    pub agreed_rate: Option<rust_decimal::Decimal>,
    /// VARCHAR(20)
    pub rate_unit: Option<String>,
    /// NUMERIC(18, 2)
    pub agreed_value: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(8, 2)
    pub estimated_hours: Option<rust_decimal::Decimal>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// DATE
    pub actual_completion_date: Option<chrono::NaiveDate>,
    /// UUID
    pub completed_confirmed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub completed_confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub project_id: Option<uuid::Uuid>,
    /// UUID
    pub production_order_id: Option<uuid::Uuid>,
    /// BOOLEAN
    pub withholding_applicable: Option<bool>,
    /// NUMERIC(6, 3)
    pub withholding_rate: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub withholding_amount: Option<rust_decimal::Decimal>,
    /// TEXT
    pub notes: Option<String>,
    /// TEXT
    pub internal_notes: Option<String>,
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

impl WorkOrder {
    pub const TABLE: &'static str = "work_orders";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "vendor_id",
        "contractor_request_id",
        "title",
        "description",
        "scope",
        "work_type",
        "payment_type",
        "agreed_rate",
        "rate_unit",
        "agreed_value",
        "currency",
        "estimated_hours",
        "status",
        "start_date",
        "end_date",
        "actual_completion_date",
        "completed_confirmed_by_id",
        "completed_confirmed_at",
        "project_id",
        "production_order_id",
        "withholding_applicable",
        "withholding_rate",
        "withholding_amount",
        "notes",
        "internal_notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
