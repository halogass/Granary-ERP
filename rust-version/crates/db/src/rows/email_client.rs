// Row structs of the email_client tables: one struct per table, its columns in order.

/// Row of `email_accounts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmailAccount {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub email_address: String,
    /// VARCHAR(255)
    pub display_name: Option<String>,
    /// VARCHAR(255) NOT NULL
    pub imap_host: String,
    /// INTEGER
    pub imap_port: Option<i32>,
    /// BOOLEAN
    pub imap_use_ssl: Option<bool>,
    /// VARCHAR(255) NOT NULL
    pub smtp_host: String,
    /// INTEGER
    pub smtp_port: Option<i32>,
    /// BOOLEAN
    pub smtp_use_tls: Option<bool>,
    /// VARCHAR(255) NOT NULL
    pub username: String,
    /// TEXT NOT NULL
    pub password_encrypted: String,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// BOOLEAN
    pub is_default: Option<bool>,
    /// BOOLEAN
    pub sync_enabled: Option<bool>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub last_sync_error: Option<String>,
    /// VARCHAR(100)
    pub sync_folder: Option<String>,
    /// INTEGER
    pub last_uid_synced: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl EmailAccount {
    pub const TABLE: &'static str = "email_accounts";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "email_address",
        "display_name",
        "imap_host",
        "imap_port",
        "imap_use_ssl",
        "smtp_host",
        "smtp_port",
        "smtp_use_tls",
        "username",
        "password_encrypted",
        "is_active",
        "is_default",
        "sync_enabled",
        "last_synced_at",
        "last_sync_error",
        "sync_folder",
        "last_uid_synced",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `email_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmailMessage {
    /// UUID NOT NULL
    pub account_id: uuid::Uuid,
    /// VARCHAR(500) NOT NULL
    pub message_id: String,
    /// INTEGER
    pub imap_uid: Option<i32>,
    /// VARCHAR(100)
    pub folder: Option<String>,
    /// VARCHAR(1000)
    pub subject: Option<String>,
    /// VARCHAR(500)
    pub from_address: Option<String>,
    /// VARCHAR[]
    pub to_addresses: Option<Vec<String>>,
    /// VARCHAR[]
    pub cc_addresses: Option<Vec<String>>,
    /// VARCHAR(500)
    pub reply_to: Option<String>,
    /// VARCHAR(500)
    pub in_reply_to: Option<String>,
    /// VARCHAR(500)
    pub thread_id: Option<String>,
    /// TEXT
    pub body_text: Option<String>,
    /// TEXT
    pub body_html: Option<String>,
    /// BOOLEAN
    pub is_read: Option<bool>,
    /// BOOLEAN
    pub is_starred: Option<bool>,
    /// BOOLEAN
    pub is_draft: Option<bool>,
    /// BOOLEAN
    pub is_sent: Option<bool>,
    /// BOOLEAN
    pub is_archived: Option<bool>,
    /// BOOLEAN
    pub is_deleted: Option<bool>,
    /// TIMESTAMP WITH TIME ZONE
    pub date_received: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub date_sent: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB
    pub attachments: Option<serde_json::Value>,
    /// UUID
    pub linked_contact_id: Option<uuid::Uuid>,
    /// UUID
    pub linked_deal_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl EmailMessage {
    pub const TABLE: &'static str = "email_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "account_id",
        "message_id",
        "imap_uid",
        "folder",
        "subject",
        "from_address",
        "to_addresses",
        "cc_addresses",
        "reply_to",
        "in_reply_to",
        "thread_id",
        "body_text",
        "body_html",
        "is_read",
        "is_starred",
        "is_draft",
        "is_sent",
        "is_archived",
        "is_deleted",
        "date_received",
        "date_sent",
        "attachments",
        "linked_contact_id",
        "linked_deal_id",
        "id",
        "created_at",
        "updated_at",
        "deleted_at",
    ];
}
