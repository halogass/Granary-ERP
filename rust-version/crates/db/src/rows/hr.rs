// Row structs of the hr tables: one struct per table, its columns in order.

/// Row of `applicants`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Applicant {
    /// UUID NOT NULL
    pub job_opening_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(255)
    pub email: Option<String>,
    /// VARCHAR(50)
    pub phone: Option<String>,
    /// VARCHAR(50)
    pub whatsapp: Option<String>,
    /// VARCHAR(255)
    pub linkedin_url: Option<String>,
    /// VARCHAR(255)
    pub portfolio_url: Option<String>,
    /// VARCHAR(50)
    pub stage: Option<String>,
    /// VARCHAR(500)
    pub resume_url: Option<String>,
    /// TEXT
    pub cover_letter: Option<String>,
    /// VARCHAR(100)
    pub source: Option<String>,
    /// UUID
    pub referred_by_id: Option<uuid::Uuid>,
    /// INTEGER
    pub rating: Option<i32>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(255)
    pub rejection_reason: Option<String>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
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

impl Applicant {
    pub const TABLE: &'static str = "applicants";
    pub const COLUMNS: &'static [&'static str] = &[
        "job_opening_id",
        "name",
        "email",
        "phone",
        "whatsapp",
        "linkedin_url",
        "portfolio_url",
        "stage",
        "resume_url",
        "cover_letter",
        "source",
        "referred_by_id",
        "rating",
        "notes",
        "rejection_reason",
        "assigned_to_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `attendance_records`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AttendanceRecord {
    /// UUID NOT NULL
    pub employee_id: uuid::Uuid,
    /// DATE NOT NULL
    pub date: chrono::NaiveDate,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(10)
    pub check_in: Option<String>,
    /// VARCHAR(10)
    pub check_out: Option<String>,
    /// NUMERIC(4, 2)
    pub work_hours: Option<rust_decimal::Decimal>,
    /// INTEGER
    pub late_minutes: Option<i32>,
    /// NUMERIC(4, 2)
    pub overtime_hours: Option<rust_decimal::Decimal>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub recorded_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AttendanceRecord {
    pub const TABLE: &'static str = "attendance_records";
    pub const COLUMNS: &'static [&'static str] = &[
        "employee_id",
        "date",
        "status",
        "check_in",
        "check_out",
        "work_hours",
        "late_minutes",
        "overtime_hours",
        "notes",
        "recorded_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `clock_events`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClockEvent {
    /// UUID
    pub employee_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub event_type: String,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub captured_at: chrono::DateTime<chrono::Utc>,
    /// VARCHAR(30) NOT NULL
    pub recognition_status: String,
    /// VARCHAR(100)
    pub recognition_uid: Option<String>,
    /// FLOAT
    pub recognition_confidence: Option<f64>,
    /// FLOAT
    pub liveness_score: Option<f64>,
    /// BOOLEAN
    pub is_real_face: Option<bool>,
    /// NUMERIC(10, 7)
    pub latitude: Option<rust_decimal::Decimal>,
    /// NUMERIC(10, 7)
    pub longitude: Option<rust_decimal::Decimal>,
    /// FLOAT
    pub geo_accuracy_meters: Option<f64>,
    /// UUID
    pub office_location_id: Option<uuid::Uuid>,
    /// FLOAT
    pub geo_distance_meters: Option<f64>,
    /// UUID
    pub override_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub override_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub override_notes: Option<String>,
    /// UUID
    pub attendance_record_id: Option<uuid::Uuid>,
    /// BOOLEAN
    pub is_kiosk: Option<bool>,
    /// UUID
    pub kiosk_device_id: Option<uuid::Uuid>,
    /// VARCHAR(20)
    pub liveness_challenge: Option<String>,
    /// BOOLEAN
    pub liveness_passed: Option<bool>,
    /// FLOAT
    pub match_distance: Option<f64>,
    /// JSONB
    pub details: Option<serde_json::Value>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ClockEvent {
    pub const TABLE: &'static str = "clock_events";
    pub const COLUMNS: &'static [&'static str] = &[
        "employee_id",
        "event_type",
        "captured_at",
        "recognition_status",
        "recognition_uid",
        "recognition_confidence",
        "liveness_score",
        "is_real_face",
        "latitude",
        "longitude",
        "geo_accuracy_meters",
        "office_location_id",
        "geo_distance_meters",
        "override_by_id",
        "override_at",
        "override_notes",
        "attendance_record_id",
        "is_kiosk",
        "kiosk_device_id",
        "liveness_challenge",
        "liveness_passed",
        "match_distance",
        "details",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `dept_coordinative_links`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DepartmentCoordinativeLink {
    /// VARCHAR(100) NOT NULL
    pub from_department: String,
    /// VARCHAR(100) NOT NULL
    pub to_department: String,
    /// VARCHAR(255)
    pub notes: Option<String>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl DepartmentCoordinativeLink {
    pub const TABLE: &'static str = "dept_coordinative_links";
    pub const COLUMNS: &'static [&'static str] = &[
        "from_department",
        "to_department",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `employee_allowances`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmployeeAllowance {
    /// UUID NOT NULL
    pub employee_id: uuid::Uuid,
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// NUMERIC(18, 2) NOT NULL
    pub amount: rust_decimal::Decimal,
    /// VARCHAR(3) NOT NULL
    pub currency: String,
    /// BOOLEAN NOT NULL
    pub is_fixed: bool,
    /// BOOLEAN NOT NULL
    pub is_taxable: bool,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// TEXT
    pub notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl EmployeeAllowance {
    pub const TABLE: &'static str = "employee_allowances";
    pub const COLUMNS: &'static [&'static str] = &[
        "employee_id",
        "name",
        "amount",
        "currency",
        "is_fixed",
        "is_taxable",
        "is_active",
        "notes",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `employees`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Employee {
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// VARCHAR(50)
    pub employee_number: Option<String>,
    /// VARCHAR(100) NOT NULL
    pub first_name: String,
    /// VARCHAR(100)
    pub last_name: Option<String>,
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// VARCHAR(255)
    pub personal_email: Option<String>,
    /// VARCHAR(50)
    pub phone: Option<String>,
    /// VARCHAR(50)
    pub whatsapp: Option<String>,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// VARCHAR(150)
    pub position: Option<String>,
    /// VARCHAR(30)
    pub employment_type: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// UUID
    pub reports_to_id: Option<uuid::Uuid>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub probation_end_date: Option<chrono::NaiveDate>,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// VARCHAR(255)
    pub termination_reason: Option<String>,
    /// NUMERIC(18, 2)
    pub base_salary: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub salary_currency: Option<String>,
    /// DATE
    pub salary_effective_date: Option<chrono::NaiveDate>,
    /// VARCHAR(20)
    pub id_type: Option<String>,
    /// VARCHAR(50)
    pub id_number: Option<String>,
    /// VARCHAR(50)
    pub tax_id: Option<String>,
    /// VARCHAR(50)
    pub health_insurance_number: Option<String>,
    /// VARCHAR(50)
    pub social_security_number: Option<String>,
    /// TEXT
    pub address: Option<String>,
    /// VARCHAR(100)
    pub city: Option<String>,
    /// VARCHAR(100)
    pub province: Option<String>,
    /// VARCHAR(255)
    pub emergency_contact_name: Option<String>,
    /// VARCHAR(50)
    pub emergency_contact_phone: Option<String>,
    /// VARCHAR(50)
    pub emergency_contact_relation: Option<String>,
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
    /// VARCHAR(30): the tax status of payroll.income_tax allowances (None = the default status)
    pub tax_status: Option<String>,
}

impl Employee {
    pub const TABLE: &'static str = "employees";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "employee_number",
        "first_name",
        "last_name",
        "email",
        "personal_email",
        "phone",
        "whatsapp",
        "department",
        "position",
        "employment_type",
        "status",
        "reports_to_id",
        "start_date",
        "probation_end_date",
        "end_date",
        "termination_reason",
        "base_salary",
        "salary_currency",
        "salary_effective_date",
        "id_type",
        "id_number",
        "tax_id",
        "health_insurance_number",
        "social_security_number",
        "address",
        "city",
        "province",
        "emergency_contact_name",
        "emergency_contact_phone",
        "emergency_contact_relation",
        "notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
        "tax_status",
    ];
}

/// Row of `face_descriptors`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FaceDescriptor {
    /// UUID NOT NULL
    pub employee_id: uuid::Uuid,
    /// FLOAT[] NOT NULL
    pub descriptor: Vec<f64>,
    /// VARCHAR(40) NOT NULL
    pub model: String,
    /// UUID
    pub enrolled_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl FaceDescriptor {
    pub const TABLE: &'static str = "face_descriptors";
    pub const COLUMNS: &'static [&'static str] = &[
        "employee_id",
        "descriptor",
        "model",
        "enrolled_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `face_enrollments`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FaceEnrollment {
    /// UUID NOT NULL
    pub employee_id: uuid::Uuid,
    /// VARCHAR(100)
    pub face_uid: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub enrolled_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub enrolled_by_id: Option<uuid::Uuid>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// INTEGER
    pub image_count: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl FaceEnrollment {
    pub const TABLE: &'static str = "face_enrollments";
    pub const COLUMNS: &'static [&'static str] = &[
        "employee_id",
        "face_uid",
        "enrolled_at",
        "enrolled_by_id",
        "is_active",
        "image_count",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `hiring_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HiringRequest {
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// VARCHAR(100) NOT NULL
    pub department: String,
    /// VARCHAR(30)
    pub employment_type: Option<String>,
    /// UUID NOT NULL
    pub requested_by_id: uuid::Uuid,
    /// UUID
    pub reports_to_employee_id: Option<uuid::Uuid>,
    /// BOOLEAN
    pub budget_approved: Option<bool>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// VARCHAR(20)
    pub urgency: Option<String>,
    /// DATE
    pub target_start_date: Option<chrono::NaiveDate>,
    /// INTEGER
    pub headcount: Option<i32>,
    /// TEXT
    pub job_description: Option<String>,
    /// TEXT
    pub requirements: Option<String>,
    /// NUMERIC(18, 2)
    pub salary_budget_min: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub salary_budget_max: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub salary_currency: Option<String>,
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

impl HiringRequest {
    pub const TABLE: &'static str = "hiring_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "title",
        "department",
        "employment_type",
        "requested_by_id",
        "reports_to_employee_id",
        "budget_approved",
        "approved_by_id",
        "approved_at",
        "status",
        "urgency",
        "target_start_date",
        "headcount",
        "job_description",
        "requirements",
        "salary_budget_min",
        "salary_budget_max",
        "salary_currency",
        "notes",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `job_openings`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JobOpening {
    /// UUID
    pub hiring_request_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// VARCHAR(30)
    pub employment_type: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// TEXT
    pub requirements: Option<String>,
    /// TEXT
    pub responsibilities: Option<String>,
    /// TEXT
    pub benefits: Option<String>,
    /// NUMERIC(18, 2)
    pub salary_min: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub salary_max: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub salary_currency: Option<String>,
    /// BOOLEAN
    pub show_salary: Option<bool>,
    /// VARCHAR(100)
    pub location: Option<String>,
    /// BOOLEAN
    pub is_remote: Option<bool>,
    /// TIMESTAMP WITH TIME ZONE
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    /// DATE
    pub closing_date: Option<chrono::NaiveDate>,
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

impl JobOpening {
    pub const TABLE: &'static str = "job_openings";
    pub const COLUMNS: &'static [&'static str] = &[
        "hiring_request_id",
        "title",
        "department",
        "employment_type",
        "status",
        "description",
        "requirements",
        "responsibilities",
        "benefits",
        "salary_min",
        "salary_max",
        "salary_currency",
        "show_salary",
        "location",
        "is_remote",
        "published_at",
        "closing_date",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `kiosk_devices`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KioskDevice {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// UUID NOT NULL
    pub office_location_id: uuid::Uuid,
    /// VARCHAR(64) NOT NULL
    pub token_hash: String,
    /// VARCHAR(12) NOT NULL
    pub token_prefix: String,
    /// UUID
    pub registered_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl KioskDevice {
    pub const TABLE: &'static str = "kiosk_devices";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "office_location_id",
        "token_hash",
        "token_prefix",
        "registered_by_id",
        "last_seen_at",
        "revoked_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `leave_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LeaveRequest {
    /// UUID
    pub employee_id: Option<uuid::Uuid>,
    /// UUID
    pub submitted_by_id: Option<uuid::Uuid>,
    /// VARCHAR(50) NOT NULL
    pub leave_type: String,
    /// DATE NOT NULL
    pub start_date: chrono::NaiveDate,
    /// DATE NOT NULL
    pub end_date: chrono::NaiveDate,
    /// NUMERIC(4, 1) NOT NULL
    pub days: rust_decimal::Decimal,
    /// BOOLEAN
    pub half_day: Option<bool>,
    /// VARCHAR(10)
    pub half_day_period: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TEXT
    pub reason: Option<String>,
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

impl LeaveRequest {
    pub const TABLE: &'static str = "leave_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "employee_id",
        "submitted_by_id",
        "leave_type",
        "start_date",
        "end_date",
        "days",
        "half_day",
        "half_day_period",
        "status",
        "reason",
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

/// Row of `offer_letters`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OfferLetter {
    /// VARCHAR(50) NOT NULL
    pub number: String,
    /// UUID NOT NULL
    pub applicant_id: uuid::Uuid,
    /// UUID
    pub job_opening_id: Option<uuid::Uuid>,
    /// UUID
    pub hiring_request_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub position_title: String,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// UUID
    pub reports_to_employee_id: Option<uuid::Uuid>,
    /// VARCHAR(30)
    pub employment_type: Option<String>,
    /// NUMERIC(18, 2) NOT NULL
    pub salary_gross_monthly: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub salary_currency: Option<String>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// INTEGER
    pub probation_months: Option<i32>,
    /// TEXT
    pub benefits: Option<String>,
    /// TEXT
    pub other_terms: Option<String>,
    /// DATE
    pub acceptance_deadline: Option<chrono::NaiveDate>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub responded_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub candidate_response_notes: Option<String>,
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

impl OfferLetter {
    pub const TABLE: &'static str = "offer_letters";
    pub const COLUMNS: &'static [&'static str] = &[
        "number",
        "applicant_id",
        "job_opening_id",
        "hiring_request_id",
        "position_title",
        "department",
        "reports_to_employee_id",
        "employment_type",
        "salary_gross_monthly",
        "salary_currency",
        "start_date",
        "probation_months",
        "benefits",
        "other_terms",
        "acceptance_deadline",
        "status",
        "sent_at",
        "responded_at",
        "candidate_response_notes",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `office_locations`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OfficeLocation {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// TEXT
    pub address: Option<String>,
    /// NUMERIC(10, 7) NOT NULL
    pub latitude: rust_decimal::Decimal,
    /// NUMERIC(10, 7) NOT NULL
    pub longitude: rust_decimal::Decimal,
    /// INTEGER
    pub radius_meters: Option<i32>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl OfficeLocation {
    pub const TABLE: &'static str = "office_locations";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "address",
        "latitude",
        "longitude",
        "radius_meters",
        "is_active",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `payroll_lines`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PayrollLine {
    /// UUID NOT NULL
    pub payroll_run_id: uuid::Uuid,
    /// UUID NOT NULL
    pub employee_id: uuid::Uuid,
    /// NUMERIC(18, 2) NOT NULL
    pub gross_salary: rust_decimal::Decimal,
    /// NUMERIC(18, 2)
    pub allowances: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub overtime_pay: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub deductions: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub income_tax: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub taxable_gross: Option<rust_decimal::Decimal>,
    /// JSONB NOT NULL: {key: {label, employee, employer}}
    pub contributions: serde_json::Value,
    /// NUMERIC(18, 2)
    pub employee_contributions: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub employer_contributions: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2) NOT NULL
    pub net_salary: rust_decimal::Decimal,
    /// INTEGER
    pub days_worked: Option<i32>,
    /// INTEGER
    pub days_absent: Option<i32>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(100)
    pub payment_reference: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PayrollLine {
    pub const TABLE: &'static str = "payroll_lines";
    pub const COLUMNS: &'static [&'static str] = &[
        "payroll_run_id",
        "employee_id",
        "gross_salary",
        "allowances",
        "overtime_pay",
        "deductions",
        "income_tax",
        "taxable_gross",
        "contributions",
        "employee_contributions",
        "employer_contributions",
        "net_salary",
        "days_worked",
        "days_absent",
        "notes",
        "payment_reference",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `payroll_runs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PayrollRun {
    /// INTEGER NOT NULL
    pub month: i32,
    /// INTEGER NOT NULL
    pub year: i32,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// NUMERIC(18, 2)
    pub total_gross: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub total_income_tax: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub total_contributions: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub total_net: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub calculated_by_id: Option<uuid::Uuid>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub paid_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub bank_account_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PayrollRun {
    pub const TABLE: &'static str = "payroll_runs";
    pub const COLUMNS: &'static [&'static str] = &[
        "month",
        "year",
        "status",
        "total_gross",
        "total_income_tax",
        "total_contributions",
        "total_net",
        "currency",
        "notes",
        "calculated_by_id",
        "approved_by_id",
        "approved_at",
        "paid_at",
        "bank_account_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `termination_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminationRequest {
    /// UUID NOT NULL
    pub employee_id: uuid::Uuid,
    /// UUID NOT NULL
    pub initiated_by_id: uuid::Uuid,
    /// UUID
    pub confirmed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(50)
    pub termination_type: Option<String>,
    /// DATE
    pub termination_date: Option<chrono::NaiveDate>,
    /// DATE
    pub last_working_date: Option<chrono::NaiveDate>,
    /// DATE
    pub notice_given_date: Option<chrono::NaiveDate>,
    /// TEXT
    pub reason: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// BOOLEAN
    pub exit_interview_done: Option<bool>,
    /// BOOLEAN
    pub assets_returned: Option<bool>,
    /// BOOLEAN
    pub accounts_deactivated: Option<bool>,
    /// BOOLEAN
    pub final_payment_processed: Option<bool>,
    /// TEXT
    pub notes: Option<String>,
    /// TEXT
    pub hr_notes: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TerminationRequest {
    pub const TABLE: &'static str = "termination_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "employee_id",
        "initiated_by_id",
        "confirmed_by_id",
        "confirmed_at",
        "termination_type",
        "termination_date",
        "last_working_date",
        "notice_given_date",
        "reason",
        "status",
        "exit_interview_done",
        "assets_returned",
        "accounts_deactivated",
        "final_payment_processed",
        "notes",
        "hr_notes",
        "id",
        "created_at",
        "updated_at",
    ];
}
