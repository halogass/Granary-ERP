// Row structs of the marketing tables: one struct per table, its columns in order.

/// Row of `content_calendar`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContentCalendarItem {
    /// UUID
    pub campaign_id: Option<uuid::Uuid>,
    /// VARCHAR(500) NOT NULL
    pub title: String,
    /// VARCHAR(50)
    pub content_type: Option<String>,
    /// VARCHAR(50)
    pub platform: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// DATE
    pub planned_date: Option<chrono::NaiveDate>,
    /// TIMESTAMP WITH TIME ZONE
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub content_body: Option<String>,
    /// VARCHAR[]
    pub media_urls: Option<Vec<String>>,
    /// VARCHAR(500)
    pub external_url: Option<String>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// INTEGER
    pub views: Option<i32>,
    /// INTEGER
    pub clicks: Option<i32>,
    /// INTEGER
    pub leads_from_content: Option<i32>,
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

impl ContentCalendarItem {
    pub const TABLE: &'static str = "content_calendar";
    pub const COLUMNS: &'static [&'static str] = &[
        "campaign_id",
        "title",
        "content_type",
        "platform",
        "status",
        "planned_date",
        "published_at",
        "content_body",
        "media_urls",
        "external_url",
        "assigned_to_id",
        "approved_by_id",
        "views",
        "clicks",
        "leads_from_content",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `email_campaigns`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmailCampaign {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(500) NOT NULL
    pub subject: String,
    /// VARCHAR(255)
    pub from_name: Option<String>,
    /// VARCHAR(255)
    pub from_email: Option<String>,
    /// JSONB
    pub blocks: Option<serde_json::Value>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB
    pub list_ids: Option<serde_json::Value>,
    /// INTEGER
    pub total_recipients: Option<i32>,
    /// INTEGER
    pub sent_count: Option<i32>,
    /// INTEGER
    pub failed_count: Option<i32>,
    /// INTEGER
    pub open_count: Option<i32>,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID
    pub sender_user_id: Option<uuid::Uuid>,
    /// BOOLEAN NOT NULL
    pub include_signature: bool,
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

impl EmailCampaign {
    pub const TABLE: &'static str = "email_campaigns";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "subject",
        "from_name",
        "from_email",
        "blocks",
        "status",
        "scheduled_at",
        "sent_at",
        "list_ids",
        "total_recipients",
        "sent_count",
        "failed_count",
        "open_count",
        "created_by_id",
        "sender_user_id",
        "include_signature",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `email_queue_items`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmailQueueItem {
    /// UUID NOT NULL
    pub campaign_id: uuid::Uuid,
    /// UUID NOT NULL
    pub list_id: uuid::Uuid,
    /// UUID NOT NULL
    pub contact_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// VARCHAR(100)
    pub first_name: Option<String>,
    /// VARCHAR(255)
    pub full_name: Option<String>,
    /// VARCHAR(255)
    pub company: Option<String>,
    /// VARCHAR(36) NOT NULL
    pub tracking_id: String,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// VARCHAR(500)
    pub error_message: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub queued_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub opened_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl EmailQueueItem {
    pub const TABLE: &'static str = "email_queue_items";
    pub const COLUMNS: &'static [&'static str] = &[
        "campaign_id",
        "list_id",
        "contact_id",
        "email",
        "first_name",
        "full_name",
        "company",
        "tracking_id",
        "status",
        "error_message",
        "queued_at",
        "sent_at",
        "opened_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `email_suppressions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmailSuppression {
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// VARCHAR(30) NOT NULL
    pub reason: String,
    /// UUID
    pub campaign_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl EmailSuppression {
    pub const TABLE: &'static str = "email_suppressions";
    pub const COLUMNS: &'static [&'static str] = &[
        "email",
        "reason",
        "campaign_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `lead_attributions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LeadAttribution {
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub campaign_id: Option<uuid::Uuid>,
    /// UUID
    pub content_item_id: Option<uuid::Uuid>,
    /// VARCHAR(100)
    pub utm_source: Option<String>,
    /// VARCHAR(100)
    pub utm_medium: Option<String>,
    /// VARCHAR(100)
    pub utm_campaign: Option<String>,
    /// VARCHAR(100)
    pub utm_content: Option<String>,
    /// VARCHAR(100)
    pub utm_term: Option<String>,
    /// VARCHAR(500)
    pub referrer_url: Option<String>,
    /// VARCHAR(500)
    pub landing_page: Option<String>,
    /// VARCHAR(30)
    pub attribution_type: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl LeadAttribution {
    pub const TABLE: &'static str = "lead_attributions";
    pub const COLUMNS: &'static [&'static str] = &[
        "lead_id",
        "campaign_id",
        "content_item_id",
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_content",
        "utm_term",
        "referrer_url",
        "landing_page",
        "attribution_type",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `mailing_list_contacts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MailingListContact {
    /// UUID NOT NULL
    pub list_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// VARCHAR(100)
    pub first_name: Option<String>,
    /// VARCHAR(100)
    pub last_name: Option<String>,
    /// VARCHAR(255)
    pub company: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_subscribed: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub unsubscribed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl MailingListContact {
    pub const TABLE: &'static str = "mailing_list_contacts";
    pub const COLUMNS: &'static [&'static str] = &[
        "list_id",
        "email",
        "first_name",
        "last_name",
        "company",
        "is_subscribed",
        "unsubscribed_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `mailing_lists`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MailingList {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// TEXT
    pub description: Option<String>,
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

impl MailingList {
    pub const TABLE: &'static str = "mailing_lists";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "description",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}

/// Row of `marketing_campaigns`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MarketingCampaign {
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(50)
    pub channel: Option<String>,
    /// VARCHAR(100)
    pub objective: Option<String>,
    /// VARCHAR(30)
    pub status: Option<String>,
    /// NUMERIC(18, 2)
    pub budget: Option<rust_decimal::Decimal>,
    /// NUMERIC(18, 2)
    pub actual_spend: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// DATE
    pub start_date: Option<chrono::NaiveDate>,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// TEXT
    pub target_audience: Option<String>,
    /// TEXT
    pub description: Option<String>,
    /// TEXT
    pub goals: Option<String>,
    /// VARCHAR(100)
    pub utm_source: Option<String>,
    /// VARCHAR(100)
    pub utm_medium: Option<String>,
    /// VARCHAR(100)
    pub utm_campaign: Option<String>,
    /// INTEGER
    pub leads_generated: Option<i32>,
    /// INTEGER
    pub deals_influenced: Option<i32>,
    /// NUMERIC(18, 2)
    pub revenue_influenced: Option<rust_decimal::Decimal>,
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

impl MarketingCampaign {
    pub const TABLE: &'static str = "marketing_campaigns";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "channel",
        "objective",
        "status",
        "budget",
        "actual_spend",
        "currency",
        "start_date",
        "end_date",
        "target_audience",
        "description",
        "goals",
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "leads_generated",
        "deals_influenced",
        "revenue_influenced",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
