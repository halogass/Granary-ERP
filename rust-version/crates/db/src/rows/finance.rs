// Row structs of the finance tables: one struct per table, its columns in order.

/// Row of `accounts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Account {
    /// VARCHAR(20) NOT NULL
    pub code: String,
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// VARCHAR(20) NOT NULL
    pub category: String,
    /// VARCHAR(50)
    pub account_type: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// VARCHAR(6)
    pub normal_balance: Option<String>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// BOOLEAN
    pub allow_direct_posting: Option<bool>,
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

impl Account {
    pub const TABLE: &'static str = "accounts";
    pub const COLUMNS: &'static [&'static str] = &[
        "code",
        "name",
        "category",
        "account_type",
        "description",
        "is_active",
        "normal_balance",
        "currency",
        "allow_direct_posting",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `bank_accounts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BankAccount {
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// VARCHAR(100) NOT NULL
    pub bank_name: String,
    /// VARCHAR(100)
    pub account_number: Option<String>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// UUID
    pub gl_account_id: Option<uuid::Uuid>,
    /// NUMERIC(18, 2)
    pub current_balance: Option<rust_decimal::Decimal>,
    /// BOOLEAN
    pub is_active: Option<bool>,
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

impl BankAccount {
    pub const TABLE: &'static str = "bank_accounts";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "bank_name",
        "account_number",
        "currency",
        "gl_account_id",
        "current_balance",
        "is_active",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `bank_statement_lines`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BankStatementLine {
    /// UUID NOT NULL
    pub statement_id: uuid::Uuid,
    /// DATE NOT NULL
    pub transaction_date: chrono::NaiveDate,
    /// VARCHAR(500)
    pub description: Option<String>,
    /// VARCHAR(200)
    pub reference: Option<String>,
    /// NUMERIC(18, 2)
    pub debit: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub credit: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub balance: Option<rust_decimal::Decimal>,
    /// BOOLEAN
    pub is_matched: Option<bool>,
    /// BOOLEAN NOT NULL
    pub is_excluded: bool,
    /// UUID
    pub matched_journal_line_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub matched_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub matched_by_id: Option<uuid::Uuid>,
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

impl BankStatementLine {
    pub const TABLE: &'static str = "bank_statement_lines";
    pub const COLUMNS: &'static [&'static str] = &[
        "statement_id",
        "transaction_date",
        "description",
        "reference",
        "debit",
        "credit",
        "balance",
        "is_matched",
        "is_excluded",
        "matched_journal_line_id",
        "matched_at",
        "matched_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `bank_statements`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BankStatement {
    /// UUID NOT NULL
    pub bank_account_id: uuid::Uuid,
    /// DATE
    pub period_start: Option<chrono::NaiveDate>,
    /// DATE
    pub period_end: Option<chrono::NaiveDate>,
    /// NUMERIC(18, 2)
    pub opening_balance: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub closing_balance: Option<rust_decimal::Decimal>,
    /// VARCHAR(500)
    pub pdf_url: Option<String>,
    /// VARCHAR(20)
    pub status: Option<String>,
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

impl BankStatement {
    pub const TABLE: &'static str = "bank_statements";
    pub const COLUMNS: &'static [&'static str] = &[
        "bank_account_id",
        "period_start",
        "period_end",
        "opening_balance",
        "closing_balance",
        "pdf_url",
        "status",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `exchange_rates`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExchangeRate {
    /// VARCHAR(3) NOT NULL
    pub from_currency: String,
    /// VARCHAR(3) NOT NULL
    pub to_currency: String,
    /// NUMERIC(24, 12) NOT NULL
    pub rate: rust_decimal::Decimal,
    /// DATE NOT NULL
    pub effective_date: chrono::NaiveDate,
    /// VARCHAR(50)
    pub source: Option<String>,
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

impl ExchangeRate {
    pub const TABLE: &'static str = "exchange_rates";
    pub const COLUMNS: &'static [&'static str] = &[
        "from_currency",
        "to_currency",
        "rate",
        "effective_date",
        "source",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `expenses`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Expense {
    /// VARCHAR(100) NOT NULL
    pub category: String,
    /// VARCHAR(500) NOT NULL
    pub description: String,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 6)
    pub exchange_rate: Option<rust_decimal::Decimal>,
    /// DATE NOT NULL
    pub expense_date: chrono::NaiveDate,
    /// UUID
    pub project_id: Option<uuid::Uuid>,
    /// UUID
    pub vendor_id: Option<uuid::Uuid>,
    /// UUID
    pub work_order_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(500)
    pub receipt_url: Option<String>,
    /// VARCHAR(100)
    pub invoice_number: Option<String>,
    /// BOOLEAN
    pub tax_deductible: Option<bool>,
    /// BOOLEAN
    pub withholding_applicable: Option<bool>,
    /// NUMERIC(6, 3)
    pub withholding_rate: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub withholding_amount: Option<rust_decimal::Decimal>,
    /// UUID
    pub submitted_by_id: Option<uuid::Uuid>,
    /// UUID
    pub employee_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub reimbursed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub bank_account_id: Option<uuid::Uuid>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
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
    /// BOOLEAN NOT NULL
    pub is_deleted: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Expense {
    pub const TABLE: &'static str = "expenses";
    pub const COLUMNS: &'static [&'static str] = &[
        "category",
        "description",
        "amount",
        "currency",
        "exchange_rate",
        "expense_date",
        "project_id",
        "vendor_id",
        "work_order_id",
        "status",
        "receipt_url",
        "invoice_number",
        "tax_deductible",
        "withholding_applicable",
        "withholding_rate",
        "withholding_amount",
        "submitted_by_id",
        "employee_id",
        "reimbursed_at",
        "bank_account_id",
        "approved_by_id",
        "approved_at",
        "rejection_reason",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `fiscal_periods`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FiscalPeriod {
    /// INTEGER NOT NULL
    pub year: i32,
    /// INTEGER NOT NULL
    pub month: i32,
    /// VARCHAR(50)
    pub name: Option<String>,
    /// DATE NOT NULL
    pub start_date: chrono::NaiveDate,
    /// DATE NOT NULL
    pub end_date: chrono::NaiveDate,
    /// BOOLEAN
    pub is_locked: Option<bool>,
    /// UUID
    pub locked_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub locked_at: Option<chrono::DateTime<chrono::Utc>>,
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

impl FiscalPeriod {
    pub const TABLE: &'static str = "fiscal_periods";
    pub const COLUMNS: &'static [&'static str] = &[
        "year",
        "month",
        "name",
        "start_date",
        "end_date",
        "is_locked",
        "locked_by_id",
        "locked_at",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `gl_account_roles`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlAccountRole {
    /// VARCHAR(100) NOT NULL
    pub role: String,
    /// UUID
    pub account_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl GlAccountRole {
    pub const TABLE: &'static str = "gl_account_roles";
    pub const COLUMNS: &'static [&'static str] =
        &["role", "account_id", "id", "created_at", "updated_at"];
}

/// Row of `invoice_items`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InvoiceItem {
    /// UUID NOT NULL
    pub invoice_id: uuid::Uuid,
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
    /// NUMERIC(5, 2) NOT NULL
    pub discount_pct: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub discount_amount: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// UUID
    pub tax_id: Option<uuid::Uuid>,
    /// VARCHAR(50)
    pub tax_code: Option<String>,
    /// NUMERIC(8, 4) NOT NULL
    pub tax_pct: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub tax_amount: rust_decimal::Decimal,
    /// VARCHAR(500)
    pub image_url: Option<String>,
    /// UUID
    pub parent_item_id: Option<uuid::Uuid>,
    /// INTEGER
    pub term_months: Option<i32>,
    /// VARCHAR(30)
    pub price_rule: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl InvoiceItem {
    pub const TABLE: &'static str = "invoice_items";
    pub const COLUMNS: &'static [&'static str] = &[
        "invoice_id",
        "product_id",
        "sort_order",
        "sku",
        "name",
        "description",
        "quantity",
        "unit",
        "unit_price",
        "currency",
        "discount_pct",
        "discount_amount",
        "amount",
        "tax_id",
        "tax_code",
        "tax_pct",
        "tax_amount",
        "image_url",
        "parent_item_id",
        "term_months",
        "price_rule",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `invoice_refunds`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InvoiceRefund {
    /// UUID NOT NULL
    pub invoice_id: uuid::Uuid,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// VARCHAR(3) NOT NULL
    pub currency: String,
    /// NUMERIC(18, 6) NOT NULL
    pub exchange_rate: rust_decimal::Decimal,
    /// DATE NOT NULL
    pub paid_date: chrono::NaiveDate,
    /// UUID
    pub bank_account_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub reference: Option<String>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl InvoiceRefund {
    pub const TABLE: &'static str = "invoice_refunds";
    pub const COLUMNS: &'static [&'static str] = &[
        "invoice_id",
        "amount",
        "currency",
        "exchange_rate",
        "paid_date",
        "bank_account_id",
        "reference",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `invoices`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Invoice {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// VARCHAR(30)
    pub invoice_type: Option<String>,
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
    /// UUID
    pub credit_of_invoice_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub project_id: Option<uuid::Uuid>,
    /// UUID
    pub quotation_id: Option<uuid::Uuid>,
    /// VARCHAR(50)
    pub ref_pi_number: Option<String>,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// UUID
    pub customer_contact_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub customer_name: Option<String>,
    /// TEXT
    pub customer_address: Option<String>,
    /// VARCHAR(100)
    pub customer_tax_id: Option<String>,
    /// VARCHAR(50)
    pub customer_tel: Option<String>,
    /// VARCHAR(255)
    pub customer_email: Option<String>,
    /// VARCHAR(255)
    pub customer_contact_person: Option<String>,
    /// VARCHAR(255)
    pub customer_contact_email: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 6)
    pub exchange_rate: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub subtotal: Option<rust_decimal::Decimal>,
    /// JSONB
    pub tax_lines: Option<serde_json::Value>,
    /// NUMERIC(18, 2)
    pub tax_total: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub total: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub amount_paid: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub amount_due: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2) NOT NULL
    pub refund_due: rust_decimal::Decimal,
    /// DATE NOT NULL
    pub issue_date: chrono::NaiveDate,
    /// DATE
    pub due_date: Option<chrono::NaiveDate>,
    /// TIMESTAMP WITH TIME ZONE
    pub paid_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(100)
    pub payment_method: Option<String>,
    /// VARCHAR(255)
    pub payment_reference: Option<String>,
    /// TEXT
    pub payment_terms_text: Option<String>,
    /// VARCHAR(20)
    pub incoterms: Option<String>,
    /// VARCHAR(255)
    pub incoterms_destination: Option<String>,
    /// VARCHAR(100)
    pub carrier: Option<String>,
    /// VARCHAR(100)
    pub tracking_number: Option<String>,
    /// VARCHAR(50)
    pub hs_code: Option<String>,
    /// VARCHAR(100)
    pub country_of_origin: Option<String>,
    /// VARCHAR(100)
    pub shipped_from: Option<String>,
    /// VARCHAR(500)
    pub pdf_url: Option<String>,
    /// UUID
    pub authorized_by_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub authorized_by_name: Option<String>,
    /// VARCHAR(255)
    pub authorized_by_title: Option<String>,
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

impl Invoice {
    pub const TABLE: &'static str = "invoices";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "invoice_type",
        "revision",
        "full_number",
        "revision_group",
        "previous_version_id",
        "revision_reason",
        "credit_of_invoice_id",
        "deal_id",
        "project_id",
        "quotation_id",
        "ref_pi_number",
        "customer_org_id",
        "customer_contact_id",
        "customer_name",
        "customer_address",
        "customer_tax_id",
        "customer_tel",
        "customer_email",
        "customer_contact_person",
        "customer_contact_email",
        "status",
        "currency",
        "exchange_rate",
        "subtotal",
        "tax_lines",
        "tax_total",
        "total",
        "amount_paid",
        "amount_due",
        "refund_due",
        "issue_date",
        "due_date",
        "paid_at",
        "payment_method",
        "payment_reference",
        "payment_terms_text",
        "incoterms",
        "incoterms_destination",
        "carrier",
        "tracking_number",
        "hs_code",
        "country_of_origin",
        "shipped_from",
        "pdf_url",
        "authorized_by_id",
        "authorized_by_name",
        "authorized_by_title",
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

/// Row of `journal_entries`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    /// VARCHAR(50) NOT NULL
    pub entry_number: String,
    /// DATE NOT NULL
    pub entry_date: chrono::NaiveDate,
    /// UUID
    pub fiscal_period_id: Option<uuid::Uuid>,
    /// TEXT NOT NULL
    pub narration: String,
    /// VARCHAR(200)
    pub reference: Option<String>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// VARCHAR(50)
    pub source: Option<String>,
    /// VARCHAR(100)
    pub source_id: Option<String>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 6)
    pub exchange_rate: Option<rust_decimal::Decimal>,
    /// UUID
    pub posted_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub posted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub reversed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub reversed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub reversal_entry_id: Option<uuid::Uuid>,
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

impl JournalEntry {
    pub const TABLE: &'static str = "journal_entries";
    pub const COLUMNS: &'static [&'static str] = &[
        "entry_number",
        "entry_date",
        "fiscal_period_id",
        "narration",
        "reference",
        "status",
        "source",
        "source_id",
        "currency",
        "exchange_rate",
        "posted_by_id",
        "posted_at",
        "reversed_by_id",
        "reversed_at",
        "reversal_entry_id",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `journal_lines`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JournalLine {
    /// UUID NOT NULL
    pub entry_id: uuid::Uuid,
    /// UUID NOT NULL
    pub account_id: uuid::Uuid,
    /// VARCHAR(500)
    pub description: Option<String>,
    /// NUMERIC(18, 2) NOT NULL
    pub debit: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub credit: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 2)
    pub amount_currency: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub sort_order: Option<i32>,
    /// UUID
    pub bank_statement_line_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl JournalLine {
    pub const TABLE: &'static str = "journal_lines";
    pub const COLUMNS: &'static [&'static str] = &[
        "entry_id",
        "account_id",
        "description",
        "debit",
        "credit",
        "currency",
        "amount_currency",
        "sort_order",
        "bank_statement_line_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `payment_schedules`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PaymentSchedule {
    /// UUID
    pub quotation_id: Option<uuid::Uuid>,
    /// UUID
    pub invoice_id: Option<uuid::Uuid>,
    /// UUID
    pub project_id: Option<uuid::Uuid>,
    /// UUID
    pub milestone_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// NUMERIC(5, 2)
    pub percentage: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub amount: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// INTEGER NOT NULL
    pub sequence: i32,
    /// VARCHAR(50)
    pub due_trigger: Option<String>,
    /// DATE
    pub due_date: Option<chrono::NaiveDate>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub proforma_invoice_id: Option<uuid::Uuid>,
    /// DATE
    pub paid_at: Option<chrono::NaiveDate>,
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

impl PaymentSchedule {
    pub const TABLE: &'static str = "payment_schedules";
    pub const COLUMNS: &'static [&'static str] = &[
        "quotation_id",
        "invoice_id",
        "project_id",
        "milestone_id",
        "name",
        "percentage",
        "amount",
        "currency",
        "sequence",
        "due_trigger",
        "due_date",
        "status",
        "proforma_invoice_id",
        "paid_at",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `payments`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Payment {
    /// UUID NOT NULL
    pub invoice_id: uuid::Uuid,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// NUMERIC(18, 6)
    pub exchange_rate: Option<rust_decimal::Decimal>,
    /// VARCHAR(100)
    pub payment_method: Option<String>,
    /// VARCHAR(255)
    pub payment_reference: Option<String>,
    /// VARCHAR(100)
    pub bank_name: Option<String>,
    /// DATE NOT NULL
    pub payment_date: chrono::NaiveDate,
    /// TIMESTAMP WITH TIME ZONE
    pub received_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub confirmed_by_id: Option<uuid::Uuid>,
    /// VARCHAR(500)
    pub proof_url: Option<String>,
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

impl Payment {
    pub const TABLE: &'static str = "payments";
    pub const COLUMNS: &'static [&'static str] = &[
        "invoice_id",
        "amount",
        "currency",
        "exchange_rate",
        "payment_method",
        "payment_reference",
        "bank_name",
        "payment_date",
        "received_at",
        "status",
        "confirmed_by_id",
        "proof_url",
        "bank_account_id",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `taxes`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tax {
    /// VARCHAR(50) NOT NULL
    pub code: String,
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// VARCHAR(300)
    pub law_reference: Option<String>,
    /// NUMERIC(8, 4) NOT NULL
    pub rate: rust_decimal::Decimal,
    /// VARCHAR(30)
    pub rate_base: Option<String>,
    /// VARCHAR(50) NOT NULL
    pub tax_type: String,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// BOOLEAN
    pub applies_to_sales: Option<bool>,
    /// BOOLEAN
    pub applies_to_purchases: Option<bool>,
    /// UUID
    pub tax_payable_account_id: Option<uuid::Uuid>,
    /// UUID
    pub tax_receivable_account_id: Option<uuid::Uuid>,
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
    /// BOOLEAN NOT NULL
    pub is_deleted: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Tax {
    pub const TABLE: &'static str = "taxes";
    pub const COLUMNS: &'static [&'static str] = &[
        "code",
        "name",
        "law_reference",
        "rate",
        "rate_base",
        "tax_type",
        "is_active",
        "applies_to_sales",
        "applies_to_purchases",
        "tax_payable_account_id",
        "tax_receivable_account_id",
        "notes",
        "created_by_id",
        "updated_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
