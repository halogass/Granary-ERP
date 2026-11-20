// Row structs of the partner tables: one struct per table, its columns in order.

/// Row of `mdf_claims`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MDFClaim {
    /// UUID NOT NULL
    pub mdf_request_id: uuid::Uuid,
    /// UUID
    pub submitted_by_id: Option<uuid::Uuid>,
    /// NUMERIC(18, 2) NOT NULL
    pub claimed_amount: rust_decimal::Decimal,
    /// VARCHAR(500)
    pub proof_file_url: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// UUID
    pub reviewed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub paid_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(255)
    pub payment_reference: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl MDFClaim {
    pub const TABLE: &'static str = "mdf_claims";
    pub const COLUMNS: &'static [&'static str] = &[
        "mdf_request_id",
        "submitted_by_id",
        "claimed_amount",
        "proof_file_url",
        "description",
        "status",
        "reviewed_by_id",
        "reviewed_at",
        "paid_at",
        "payment_reference",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `mdf_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MDFRequest {
    /// UUID NOT NULL
    pub partner_id: uuid::Uuid,
    /// UUID
    pub submitted_by_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(50) NOT NULL
    pub activity_type: String,
    /// NUMERIC(18, 2) NOT NULL
    pub requested_amount: rust_decimal::Decimal,
    /// NUMERIC(18, 2)
    pub approved_amount: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// DATE
    pub planned_date: Option<chrono::NaiveDate>,
    /// VARCHAR(255)
    pub planned_location: Option<String>,
    /// TEXT
    pub expected_outcome: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
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

impl MDFRequest {
    pub const TABLE: &'static str = "mdf_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "partner_id",
        "submitted_by_id",
        "title",
        "description",
        "activity_type",
        "requested_amount",
        "approved_amount",
        "currency",
        "planned_date",
        "planned_location",
        "expected_outcome",
        "status",
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

/// Row of `partner_accounts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartnerAccount {
    /// UUID
    pub organization_id: Option<uuid::Uuid>,
    /// UUID
    pub primary_contact_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub company_name: Option<String>,
    /// VARCHAR(255)
    pub contact_name: Option<String>,
    /// VARCHAR(255)
    pub contact_email: Option<String>,
    /// VARCHAR(50)
    pub contact_phone: Option<String>,
    /// VARCHAR(100)
    pub country: Option<String>,
    /// VARCHAR(500)
    pub website: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(50)
    pub agreement_ref: Option<String>,
    /// VARCHAR(500)
    pub agreement_url: Option<String>,
    /// VARCHAR(50) NOT NULL
    pub partner_type: String,
    /// VARCHAR(30)
    pub tier: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR[]
    pub territory_countries: Option<Vec<String>>,
    /// VARCHAR[]
    pub territory_verticals: Option<Vec<String>>,
    /// VARCHAR[]
    pub authorized_products: Option<Vec<String>>,
    /// VARCHAR[]
    pub authorized_services: Option<Vec<String>>,
    /// BOOLEAN
    pub is_exclusive: Option<bool>,
    /// JSONB
    pub exclusivity_scope: Option<serde_json::Value>,
    /// INTEGER
    pub annual_min_units_y1: Option<i32>,
    /// NUMERIC(18, 2)
    pub annual_min_revenue_y1: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub annual_min_units_y2: Option<i32>,
    /// NUMERIC(18, 2)
    pub annual_min_revenue_y2: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub annual_min_units_y3: Option<i32>,
    /// NUMERIC(18, 2)
    pub annual_min_revenue_y3: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub commitment_currency: Option<String>,
    /// INTEGER
    pub certified_staff_count: Option<i32>,
    /// JSONB
    pub certification_details: Option<serde_json::Value>,
    /// INTEGER
    pub cumulative_units: Option<i32>,
    /// NUMERIC(18, 2)
    pub cumulative_revenue: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub current_year_units: Option<i32>,
    /// NUMERIC(18, 2)
    pub current_year_revenue: Option<rust_decimal::Decimal>,
    /// DATE
    pub tier_reviewed_at: Option<chrono::NaiveDate>,
    /// TIMESTAMP WITH TIME ZONE
    pub agreement_signed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// DATE
    pub agreement_expires_at: Option<chrono::NaiveDate>,
    /// DATE
    pub joined_at: Option<chrono::NaiveDate>,
    /// UUID
    pub assigned_am_id: Option<uuid::Uuid>,
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

impl PartnerAccount {
    pub const TABLE: &'static str = "partner_accounts";
    pub const COLUMNS: &'static [&'static str] = &[
        "organization_id",
        "primary_contact_id",
        "company_name",
        "contact_name",
        "contact_email",
        "contact_phone",
        "country",
        "website",
        "notes",
        "agreement_ref",
        "agreement_url",
        "partner_type",
        "tier",
        "status",
        "territory_countries",
        "territory_verticals",
        "authorized_products",
        "authorized_services",
        "is_exclusive",
        "exclusivity_scope",
        "annual_min_units_y1",
        "annual_min_revenue_y1",
        "annual_min_units_y2",
        "annual_min_revenue_y2",
        "annual_min_units_y3",
        "annual_min_revenue_y3",
        "commitment_currency",
        "certified_staff_count",
        "certification_details",
        "cumulative_units",
        "cumulative_revenue",
        "current_year_units",
        "current_year_revenue",
        "tier_reviewed_at",
        "agreement_signed_at",
        "agreement_expires_at",
        "joined_at",
        "assigned_am_id",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `partner_certifications`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartnerCertification {
    /// UUID NOT NULL
    pub partner_user_id: uuid::Uuid,
    /// UUID NOT NULL
    pub module_id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE
    pub issued_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(500)
    pub certificate_url: Option<String>,
    /// VARCHAR(100)
    pub badge_slug: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PartnerCertification {
    pub const TABLE: &'static str = "partner_certifications";
    pub const COLUMNS: &'static [&'static str] = &[
        "partner_user_id",
        "module_id",
        "issued_at",
        "expires_at",
        "certificate_url",
        "badge_slug",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `partner_commissions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartnerCommission {
    /// UUID NOT NULL
    pub partner_id: uuid::Uuid,
    /// UUID
    pub deal_registration_id: Option<uuid::Uuid>,
    /// UUID
    pub invoice_id: Option<uuid::Uuid>,
    /// VARCHAR(50) NOT NULL
    pub commission_type: String,
    /// NUMERIC(18, 2) NOT NULL
    pub base_amount: rust_decimal::Decimal,
    /// NUMERIC(6, 4) NOT NULL
    pub rate: rust_decimal::Decimal,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// INTEGER
    pub period_month: Option<i32>,
    /// INTEGER
    pub period_year: Option<i32>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub paid_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(255)
    pub payment_reference: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PartnerCommission {
    pub const TABLE: &'static str = "partner_commissions";
    pub const COLUMNS: &'static [&'static str] = &[
        "partner_id",
        "deal_registration_id",
        "invoice_id",
        "commission_type",
        "base_amount",
        "rate",
        "amount",
        "currency",
        "period_month",
        "period_year",
        "status",
        "approved_by_id",
        "paid_at",
        "payment_reference",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `partner_deal_registrations`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartnerDealRegistration {
    /// VARCHAR(50)
    pub registration_number: Option<String>,
    /// UUID NOT NULL
    pub partner_id: uuid::Uuid,
    /// UUID
    pub customer_org_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub customer_org_name: Option<String>,
    /// VARCHAR(255)
    pub customer_contact_name: Option<String>,
    /// VARCHAR(255)
    pub customer_contact_email: Option<String>,
    /// VARCHAR(100)
    pub customer_country: Option<String>,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub scope: Option<String>,
    /// NUMERIC(18, 2)
    pub estimated_value: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// DATE
    pub expected_close_date: Option<chrono::NaiveDate>,
    /// VARCHAR(255)
    pub engagement_role: Option<String>,
    /// VARCHAR[]
    pub products_services: Option<Vec<String>>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub registered_at: chrono::DateTime<chrono::Utc>,
    /// INTEGER
    pub protection_days: Option<i32>,
    /// DATE
    pub expires_at: Option<chrono::NaiveDate>,
    /// DATE
    pub extended_until: Option<chrono::NaiveDate>,
    /// TIMESTAMP WITH TIME ZONE
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// VARCHAR(500)
    pub rejection_reason: Option<String>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// NUMERIC(18, 2)
    pub revenue_credited: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub revenue_credited_currency: Option<String>,
    /// INTEGER
    pub revenue_credited_year: Option<i32>,
    /// VARCHAR(30)
    pub status_before_close: Option<String>,
    /// UUID
    pub conflicting_registration_id: Option<uuid::Uuid>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PartnerDealRegistration {
    pub const TABLE: &'static str = "partner_deal_registrations";
    pub const COLUMNS: &'static [&'static str] = &[
        "registration_number",
        "partner_id",
        "customer_org_id",
        "customer_org_name",
        "customer_contact_name",
        "customer_contact_email",
        "customer_country",
        "title",
        "scope",
        "estimated_value",
        "currency",
        "expected_close_date",
        "engagement_role",
        "products_services",
        "status",
        "registered_at",
        "protection_days",
        "expires_at",
        "extended_until",
        "approved_at",
        "approved_by_id",
        "rejection_reason",
        "deal_id",
        "revenue_credited",
        "revenue_credited_currency",
        "revenue_credited_year",
        "status_before_close",
        "conflicting_registration_id",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `partner_leads`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartnerLead {
    /// UUID NOT NULL
    pub partner_id: uuid::Uuid,
    /// UUID
    pub assigned_by_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub customer_name: String,
    /// VARCHAR(255)
    pub customer_email: Option<String>,
    /// VARCHAR(255)
    pub customer_company: Option<String>,
    /// VARCHAR(100)
    pub customer_country: Option<String>,
    /// VARCHAR[]
    pub product_interest: Option<Vec<String>>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub converted_deal_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub assigned_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
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

impl PartnerLead {
    pub const TABLE: &'static str = "partner_leads";
    pub const COLUMNS: &'static [&'static str] = &[
        "partner_id",
        "assigned_by_id",
        "customer_name",
        "customer_email",
        "customer_company",
        "customer_country",
        "product_interest",
        "notes",
        "status",
        "converted_deal_id",
        "assigned_at",
        "expires_at",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `partner_module_progress`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartnerModuleProgress {
    /// UUID NOT NULL
    pub partner_user_id: uuid::Uuid,
    /// UUID NOT NULL
    pub module_id: uuid::Uuid,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// INTEGER
    pub current_chapter: Option<i32>,
    /// INTEGER
    pub score: Option<i32>,
    /// TIMESTAMP WITH TIME ZONE
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// INTEGER NOT NULL: quiz submissions so far (capped by partner.training.max_attempts)
    pub attempts: i32,
}

impl PartnerModuleProgress {
    pub const TABLE: &'static str = "partner_module_progress";
    pub const COLUMNS: &'static [&'static str] = &[
        "partner_user_id",
        "module_id",
        "status",
        "current_chapter",
        "score",
        "started_at",
        "completed_at",
        "id",
        "created_at",
        "updated_at",
        "attempts",
    ];
}

/// Row of `partner_resources`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartnerResource {
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(50)
    pub resource_type: Option<String>,
    /// VARCHAR(500)
    pub file_url: Option<String>,
    /// VARCHAR(500)
    pub thumbnail_url: Option<String>,
    /// VARCHAR(20)
    pub version: Option<String>,
    /// VARCHAR(30)
    pub min_tier: Option<String>,
    /// VARCHAR[]
    pub partner_types: Option<Vec<String>>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// INTEGER
    pub download_count: Option<i32>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PartnerResource {
    pub const TABLE: &'static str = "partner_resources";
    pub const COLUMNS: &'static [&'static str] = &[
        "title",
        "description",
        "resource_type",
        "file_url",
        "thumbnail_url",
        "version",
        "min_tier",
        "partner_types",
        "is_active",
        "download_count",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `training_chapters`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrainingChapter {
    /// UUID NOT NULL
    pub module_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub content: Option<String>,
    /// VARCHAR(500)
    pub video_url: Option<String>,
    /// INTEGER
    pub order: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TrainingChapter {
    pub const TABLE: &'static str = "training_chapters";
    pub const COLUMNS: &'static [&'static str] = &[
        "module_id",
        "title",
        "content",
        "video_url",
        "order",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `training_modules`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrainingModule {
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub description: Option<String>,
    /// VARCHAR(500)
    pub thumbnail_url: Option<String>,
    /// VARCHAR(30)
    pub min_tier: Option<String>,
    /// VARCHAR[]
    pub partner_types: Option<Vec<String>>,
    /// BOOLEAN
    pub is_published: Option<bool>,
    /// INTEGER
    pub pass_score: Option<i32>,
    /// VARCHAR(500)
    pub certificate_template_url: Option<String>,
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

impl TrainingModule {
    pub const TABLE: &'static str = "training_modules";
    pub const COLUMNS: &'static [&'static str] = &[
        "title",
        "description",
        "thumbnail_url",
        "min_tier",
        "partner_types",
        "is_published",
        "pass_score",
        "certificate_template_url",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `training_quiz_questions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrainingQuizQuestion {
    /// UUID NOT NULL
    pub chapter_id: uuid::Uuid,
    /// TEXT NOT NULL
    pub question: String,
    /// JSONB NOT NULL
    pub options: serde_json::Value,
    /// INTEGER NOT NULL
    pub correct_index: i32,
    /// INTEGER
    pub order: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TrainingQuizQuestion {
    pub const TABLE: &'static str = "training_quiz_questions";
    pub const COLUMNS: &'static [&'static str] = &[
        "chapter_id",
        "question",
        "options",
        "correct_index",
        "order",
        "id",
        "created_at",
        "updated_at",
    ];
}
