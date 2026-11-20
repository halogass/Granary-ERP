// Row structs of the crm_inbox tables: one struct per table, its columns in order.

/// Row of `crm_email_attachments`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrmEmailAttachment {
    /// UUID NOT NULL
    pub message_id: uuid::Uuid,
    /// VARCHAR(500) NOT NULL
    pub filename: String,
    /// VARCHAR(200)
    pub content_type: Option<String>,
    /// INTEGER
    pub size_bytes: Option<i32>,
    /// BYTEA NOT NULL
    pub data: Vec<u8>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CrmEmailAttachment {
    pub const TABLE: &'static str = "crm_email_attachments";
    pub const COLUMNS: &'static [&'static str] = &[
        "message_id",
        "filename",
        "content_type",
        "size_bytes",
        "data",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `crm_email_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrmEmailMessage {
    /// UUID NOT NULL
    pub thread_id: uuid::Uuid,
    /// VARCHAR(1000) NOT NULL
    pub message_id_header: String,
    /// VARCHAR(1000)
    pub in_reply_to: Option<String>,
    /// TEXT
    pub references: Option<String>,
    /// VARCHAR(10) NOT NULL
    pub direction: String,
    /// VARCHAR(500) NOT NULL
    pub from_email: String,
    /// VARCHAR(500)
    pub from_name: Option<String>,
    /// VARCHAR[]
    pub to_emails: Option<Vec<String>>,
    /// VARCHAR[]
    pub cc_emails: Option<Vec<String>>,
    /// VARCHAR(1000)
    pub subject: Option<String>,
    /// TEXT
    pub body_html: Option<String>,
    /// TEXT
    pub body_text: Option<String>,
    /// INTEGER
    pub imap_uid: Option<i32>,
    /// VARCHAR(40)
    pub imap_folder: Option<String>,
    /// VARCHAR(500)
    pub reply_to: Option<String>,
    /// JSONB
    pub auth: Option<serde_json::Value>,
    /// BOOLEAN NOT NULL
    pub is_read: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub received_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub sent_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub nary_processed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BOOLEAN NOT NULL
    pub sent_by_nary: bool,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CrmEmailMessage {
    pub const TABLE: &'static str = "crm_email_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "thread_id",
        "message_id_header",
        "in_reply_to",
        "references",
        "direction",
        "from_email",
        "from_name",
        "to_emails",
        "cc_emails",
        "subject",
        "body_html",
        "body_text",
        "imap_uid",
        "imap_folder",
        "reply_to",
        "auth",
        "is_read",
        "received_at",
        "sent_by_id",
        "nary_processed_at",
        "sent_by_nary",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `crm_email_threads`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrmEmailThread {
    /// VARCHAR(1000)
    pub subject: Option<String>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// BOOLEAN
    pub is_read: Option<bool>,
    /// INTEGER
    pub message_count: Option<i32>,
    /// VARCHAR(500)
    pub last_sender_email: Option<String>,
    /// VARCHAR(500)
    pub last_sender_name: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub linked_contact_id: Option<uuid::Uuid>,
    /// UUID
    pub linked_org_id: Option<uuid::Uuid>,
    /// UUID
    pub linked_lead_id: Option<uuid::Uuid>,
    /// UUID
    pub linked_deal_id: Option<uuid::Uuid>,
    /// UUID
    pub assigned_to_id: Option<uuid::Uuid>,
    /// UUID
    pub assigned_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub assigned_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl CrmEmailThread {
    pub const TABLE: &'static str = "crm_email_threads";
    pub const COLUMNS: &'static [&'static str] = &[
        "subject",
        "status",
        "is_read",
        "message_count",
        "last_sender_email",
        "last_sender_name",
        "last_message_at",
        "linked_contact_id",
        "linked_org_id",
        "linked_lead_id",
        "linked_deal_id",
        "assigned_to_id",
        "assigned_by_id",
        "assigned_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `wa_hidden_chats`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WhatsAppHiddenChat {
    /// VARCHAR(255) NOT NULL
    pub chat_id: String,
    /// INTEGER NOT NULL
    pub hidden_through: i32,
    /// UUID
    pub hidden_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WhatsAppHiddenChat {
    pub const TABLE: &'static str = "wa_hidden_chats";
    pub const COLUMNS: &'static [&'static str] = &[
        "chat_id",
        "hidden_through",
        "hidden_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `wa_sent_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WaSentMessage {
    /// VARCHAR(128) NOT NULL
    pub message_key: String,
    /// VARCHAR(255)
    pub chat_id: Option<String>,
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// VARCHAR(120) NOT NULL
    pub sender_name: String,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WaSentMessage {
    pub const TABLE: &'static str = "wa_sent_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "message_key",
        "chat_id",
        "user_id",
        "sender_name",
        "id",
        "created_at",
        "updated_at",
    ];
}
