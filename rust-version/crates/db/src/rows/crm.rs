// Row structs of the crm tables: one struct per table, its columns in order.

/// Row of `activities`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Activity {
    /// VARCHAR(50) NOT NULL
    pub entity_type: String,
    /// UUID NOT NULL
    pub entity_id: uuid::Uuid,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// VARCHAR(50) NOT NULL
    pub activity_type: String,
    /// VARCHAR(20)
    pub direction: Option<String>,
    /// VARCHAR(500)
    pub subject: Option<String>,
    /// TEXT
    pub content: Option<String>,
    /// VARCHAR(255)
    pub outcome: Option<String>,
    /// INTEGER
    pub duration_minutes: Option<i32>,
    /// TIMESTAMP WITH TIME ZONE
    pub scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BOOLEAN
    pub is_completed: Option<bool>,
    /// VARCHAR(500)
    pub next_action: Option<String>,
    /// DATE
    pub next_action_due: Option<chrono::NaiveDate>,
    /// VARCHAR(255)
    pub channel_message_id: Option<String>,
    /// JSONB
    pub channel_data: Option<serde_json::Value>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Activity {
    pub const TABLE: &'static str = "activities";
    pub const COLUMNS: &'static [&'static str] = &[
        "entity_type",
        "entity_id",
        "contact_id",
        "lead_id",
        "deal_id",
        "activity_type",
        "direction",
        "subject",
        "content",
        "outcome",
        "duration_minutes",
        "scheduled_at",
        "completed_at",
        "is_completed",
        "next_action",
        "next_action_due",
        "channel_message_id",
        "channel_data",
        "created_by_id",
        "assigned_to_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `contacts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Contact {
    /// UUID
    pub organization_id: Option<uuid::Uuid>,
    /// VARCHAR(100) NOT NULL
    pub first_name: String,
    /// VARCHAR(100)
    pub last_name: Option<String>,
    /// VARCHAR(255)
    pub email: Option<String>,
    /// VARCHAR(50)
    pub phone: Option<String>,
    /// VARCHAR(50)
    pub whatsapp: Option<String>,
    /// VARCHAR(150)
    pub position: Option<String>,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// BOOLEAN
    pub is_primary: Option<bool>,
    /// VARCHAR(255)
    pub linkedin_url: Option<String>,
    /// VARCHAR(100)
    pub country: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR[]
    pub tags: Option<Vec<String>>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// JSONB NOT NULL
    pub custom: serde_json::Value,
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

impl Contact {
    pub const TABLE: &'static str = "contacts";
    pub const COLUMNS: &'static [&'static str] = &[
        "organization_id",
        "first_name",
        "last_name",
        "email",
        "phone",
        "whatsapp",
        "position",
        "department",
        "is_primary",
        "linkedin_url",
        "country",
        "notes",
        "tags",
        "assigned_to_id",
        "created_by_id",
        "custom",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `crm_reminders`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrmReminder {
    /// UUID NOT NULL
    pub owner_user_id: uuid::Uuid,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub source: String,
    /// VARCHAR(30) NOT NULL
    pub entity_type: String,
    /// UUID
    pub entity_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub chat_id: Option<String>,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// TEXT
    pub note: Option<String>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub due_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub original_due_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// INTEGER NOT NULL
    pub snooze_count: i32,
    /// TIMESTAMP WITH TIME ZONE
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub completed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub notified_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub escalated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(255)
    pub rule_key: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CrmReminder {
    pub const TABLE: &'static str = "crm_reminders";
    pub const COLUMNS: &'static [&'static str] = &[
        "owner_user_id",
        "created_by_id",
        "source",
        "entity_type",
        "entity_id",
        "chat_id",
        "title",
        "note",
        "due_at",
        "original_due_at",
        "status",
        "snooze_count",
        "completed_at",
        "completed_by_id",
        "notified_at",
        "escalated_at",
        "rule_key",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `deal_stage_events`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DealStageEvent {
    /// UUID NOT NULL
    pub deal_id: uuid::Uuid,
    /// VARCHAR(50)
    pub from_stage: Option<String>,
    /// VARCHAR(50) NOT NULL
    pub to_stage: String,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub at: chrono::DateTime<chrono::Utc>,
    /// UUID
    pub by_id: Option<uuid::Uuid>,
    /// VARCHAR(30) NOT NULL
    pub origin: String,
    /// JSONB
    pub reasons: Option<serde_json::Value>,
    /// TEXT
    pub note: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl DealStageEvent {
    pub const TABLE: &'static str = "deal_stage_events";
    pub const COLUMNS: &'static [&'static str] = &[
        "deal_id",
        "from_stage",
        "to_stage",
        "at",
        "by_id",
        "origin",
        "reasons",
        "note",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `deals`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Deal {
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub organization_id: Option<uuid::Uuid>,
    /// VARCHAR(255) NOT NULL
    pub title: String,
    /// VARCHAR(50)
    pub deal_number: Option<String>,
    /// VARCHAR(50) NOT NULL
    pub stage: String,
    /// NUMERIC(18, 2)
    pub value: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// INTEGER
    pub probability: Option<i32>,
    /// DATE
    pub expected_close_date: Option<chrono::NaiveDate>,
    /// DATE
    pub actual_close_date: Option<chrono::NaiveDate>,
    /// VARCHAR[]
    pub product_interests: Option<Vec<String>>,
    /// VARCHAR[]
    pub product_line_ids: Option<Vec<String>>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(255)
    pub loss_reason: Option<String>,
    /// JSONB
    pub loss_reasons: Option<serde_json::Value>,
    /// TEXT
    pub loss_notes: Option<String>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_registration_id: Option<uuid::Uuid>,
    /// JSONB NOT NULL
    pub custom: serde_json::Value,
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
    /// VARCHAR(32): the segment (services::segment)
    #[sqlx(default)]
    #[serde(default)]
    pub segment: Option<String>,
}

impl Deal {
    pub const TABLE: &'static str = "deals";
    pub const COLUMNS: &'static [&'static str] = &[
        "lead_id",
        "contact_id",
        "organization_id",
        "title",
        "deal_number",
        "stage",
        "value",
        "currency",
        "probability",
        "expected_close_date",
        "actual_close_date",
        "product_interests",
        "product_line_ids",
        "notes",
        "loss_reason",
        "loss_reasons",
        "loss_notes",
        "assigned_to_id",
        "created_by_id",
        "deal_registration_id",
        "custom",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
        "segment",
    ];
}

/// Row of `leads`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lead {
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub organization_id: Option<uuid::Uuid>,
    /// VARCHAR(50) NOT NULL
    pub source: String,
    /// VARCHAR(255)
    pub source_detail: Option<String>,
    /// UUID
    pub source_campaign_id: Option<uuid::Uuid>,
    /// UUID
    pub partner_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub title: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// NUMERIC(18, 2)
    pub estimated_value: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// VARCHAR[]
    pub interested_products: Option<Vec<String>>,
    /// VARCHAR(50)
    pub status: Option<String>,
    /// VARCHAR(255)
    pub disqualification_reason: Option<String>,
    /// VARCHAR(20)
    pub priority: Option<String>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID
    pub converted_to_deal_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub converted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub raw_message: Option<String>,
    /// JSONB
    pub raw_channel_data: Option<serde_json::Value>,
    /// JSONB
    pub attribution: Option<serde_json::Value>,
    /// JSONB
    pub ai_score: Option<serde_json::Value>,
    /// JSONB NOT NULL
    pub custom: serde_json::Value,
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
    /// VARCHAR(32): the segment (services::segment)
    #[sqlx(default)]
    #[serde(default)]
    pub segment: Option<String>,
}

impl Lead {
    pub const TABLE: &'static str = "leads";
    pub const COLUMNS: &'static [&'static str] = &[
        "contact_id",
        "organization_id",
        "source",
        "source_detail",
        "source_campaign_id",
        "partner_id",
        "title",
        "description",
        "estimated_value",
        "currency",
        "interested_products",
        "status",
        "disqualification_reason",
        "priority",
        "assigned_to_id",
        "created_by_id",
        "converted_to_deal_id",
        "converted_at",
        "raw_message",
        "raw_channel_data",
        "attribution",
        "ai_score",
        "custom",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
        "segment",
    ];
}

/// Row of `organizations`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Organization {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    /// VARCHAR[]
    pub type_: Option<Vec<String>>,
    /// VARCHAR(100)
    pub industry: Option<String>,
    /// VARCHAR(100)
    pub country: Option<String>,
    /// VARCHAR(100)
    pub city: Option<String>,
    /// TEXT
    pub address: Option<String>,
    /// VARCHAR(20)
    pub postal_code: Option<String>,
    /// VARCHAR(255)
    pub website: Option<String>,
    /// VARCHAR(50)
    pub phone: Option<String>,
    /// VARCHAR(255)
    pub email: Option<String>,
    /// VARCHAR(100)
    pub tax_id: Option<String>,
    /// VARCHAR(100)
    pub registration_number: Option<String>,
    /// NUMERIC(18, 2)
    pub annual_revenue: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub revenue_currency: Option<String>,
    /// INTEGER
    pub employee_count: Option<i32>,
    /// TEXT
    pub description: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR[]
    pub tags: Option<Vec<String>>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// JSONB NOT NULL
    pub custom: serde_json::Value,
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
    /// BOOLEAN NOT NULL DEFAULT false (migration 0147): quoted without tax: every sales document for this
    /// customer starts with no tax instead of finance.default_sales_tax
    #[serde(default)]
    #[sqlx(default)]
    pub quote_without_tax: bool,
}

impl Organization {
    pub const TABLE: &'static str = "organizations";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "type",
        "industry",
        "country",
        "city",
        "address",
        "postal_code",
        "website",
        "phone",
        "email",
        "tax_id",
        "registration_number",
        "annual_revenue",
        "revenue_currency",
        "employee_count",
        "description",
        "notes",
        "tags",
        "assigned_to_id",
        "created_by_id",
        "custom",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
        "quote_without_tax",
    ];
}

/// Row of `scheduled_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScheduledMessage {
    /// VARCHAR(20) NOT NULL
    pub channel: String,
    /// UUID NOT NULL
    pub created_by_id: uuid::Uuid,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub chat_id: Option<String>,
    /// VARCHAR(255) NOT NULL
    pub recipient: String,
    /// VARCHAR(500)
    pub subject: Option<String>,
    /// TEXT NOT NULL
    pub body: String,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub scheduled_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// INTEGER
    pub retry_count: Option<i32>,
    /// TEXT
    pub last_error: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ScheduledMessage {
    pub const TABLE: &'static str = "scheduled_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "channel",
        "created_by_id",
        "contact_id",
        "chat_id",
        "recipient",
        "subject",
        "body",
        "scheduled_at",
        "sent_at",
        "status",
        "retry_count",
        "last_error",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `wa_contact_links`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WaContactLink {
    /// VARCHAR(50) NOT NULL
    pub phone: String,
    /// UUID NOT NULL
    pub contact_id: uuid::Uuid,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WaContactLink {
    pub const TABLE: &'static str = "wa_contact_links";
    pub const COLUMNS: &'static [&'static str] =
        &["phone", "contact_id", "id", "created_at", "updated_at"];
}
