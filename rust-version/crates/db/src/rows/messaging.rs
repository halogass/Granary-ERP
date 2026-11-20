// Row structs of the messaging tables: one struct per table, its columns in order.

/// Row of `channel_memberships`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelMembership {
    /// UUID NOT NULL
    pub channel_id: uuid::Uuid,
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(20)
    pub role: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_read_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ChannelMembership {
    pub const TABLE: &'static str = "channel_memberships";
    pub const COLUMNS: &'static [&'static str] = &[
        "channel_id",
        "user_id",
        "role",
        "last_read_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `channel_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelMessage {
    /// UUID NOT NULL
    pub channel_id: uuid::Uuid,
    /// UUID NOT NULL
    pub sender_id: uuid::Uuid,
    /// TEXT NOT NULL
    pub content: String,
    /// VARCHAR(20)
    pub message_type: Option<String>,
    /// VARCHAR(500)
    pub file_url: Option<String>,
    /// VARCHAR(255)
    pub file_name: Option<String>,
    /// INTEGER
    pub file_size: Option<i32>,
    /// VARCHAR(100)
    pub mime_type: Option<String>,
    /// UUID
    pub reply_to_id: Option<uuid::Uuid>,
    /// UUID
    pub thread_root_id: Option<uuid::Uuid>,
    /// INTEGER
    pub thread_count: Option<i32>,
    /// JSONB
    pub mentions: Option<serde_json::Value>,
    /// BOOLEAN
    pub is_deleted: Option<bool>,
    /// UUID
    pub memo_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl ChannelMessage {
    pub const TABLE: &'static str = "channel_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "channel_id",
        "sender_id",
        "content",
        "message_type",
        "file_url",
        "file_name",
        "file_size",
        "mime_type",
        "reply_to_id",
        "thread_root_id",
        "thread_count",
        "mentions",
        "is_deleted",
        "memo_id",
        "id",
        "created_at",
        "updated_at",
        "deleted_at",
    ];
}

/// Row of `channels`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// VARCHAR(100) NOT NULL
    pub slug: String,
    /// TEXT
    pub description: Option<String>,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    /// VARCHAR(20)
    pub type_: Option<String>,
    /// UUID NOT NULL
    pub created_by_id: uuid::Uuid,
    /// BOOLEAN
    pub is_archived: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Channel {
    pub const TABLE: &'static str = "channels";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "slug",
        "description",
        "type",
        "created_by_id",
        "is_archived",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `direct_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirectMessage {
    /// VARCHAR(100) NOT NULL
    pub thread_id: String,
    /// UUID NOT NULL
    pub sender_id: uuid::Uuid,
    /// UUID NOT NULL
    pub receiver_id: uuid::Uuid,
    /// TEXT NOT NULL
    pub content: String,
    /// VARCHAR(20)
    pub message_type: Option<String>,
    /// VARCHAR(500)
    pub file_url: Option<String>,
    /// VARCHAR(255)
    pub file_name: Option<String>,
    /// INTEGER
    pub file_size: Option<i32>,
    /// VARCHAR(100)
    pub mime_type: Option<String>,
    /// BOOLEAN
    pub is_read: Option<bool>,
    /// TIMESTAMP WITH TIME ZONE
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BOOLEAN
    pub is_deleted_by_sender: Option<bool>,
    /// BOOLEAN
    pub is_deleted_by_receiver: Option<bool>,
    /// UUID
    pub reply_to_id: Option<uuid::Uuid>,
    /// UUID
    pub thread_root_id: Option<uuid::Uuid>,
    /// INTEGER
    pub thread_count: Option<i32>,
    /// JSONB
    pub mentions: Option<serde_json::Value>,
    /// UUID
    pub memo_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl DirectMessage {
    pub const TABLE: &'static str = "direct_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "thread_id",
        "sender_id",
        "receiver_id",
        "content",
        "message_type",
        "file_url",
        "file_name",
        "file_size",
        "mime_type",
        "is_read",
        "read_at",
        "is_deleted_by_sender",
        "is_deleted_by_receiver",
        "reply_to_id",
        "thread_root_id",
        "thread_count",
        "mentions",
        "memo_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `memos`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Memo {
    /// VARCHAR(30) NOT NULL
    pub memo_number: String,
    /// VARCHAR(500) NOT NULL
    pub subject: String,
    /// VARCHAR(30) NOT NULL
    pub memo_type: String,
    /// TEXT NOT NULL
    pub body: String,
    /// UUID NOT NULL
    pub sender_id: uuid::Uuid,
    /// VARCHAR(255) NOT NULL
    pub sender_name: String,
    /// VARCHAR(255)
    pub sender_position: Option<String>,
    /// VARCHAR(255)
    pub sender_department: Option<String>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub sender_signed_at: chrono::DateTime<chrono::Utc>,
    /// UUID
    pub recipient_id: Option<uuid::Uuid>,
    /// UUID
    pub channel_id: Option<uuid::Uuid>,
    /// VARCHAR(30) NOT NULL
    pub status: String,
    /// TIMESTAMP WITH TIME ZONE
    pub recalled_at: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB
    pub acknowledgements: Option<serde_json::Value>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Memo {
    pub const TABLE: &'static str = "memos";
    pub const COLUMNS: &'static [&'static str] = &[
        "memo_number",
        "subject",
        "memo_type",
        "body",
        "sender_id",
        "sender_name",
        "sender_position",
        "sender_department",
        "sender_signed_at",
        "recipient_id",
        "channel_id",
        "status",
        "recalled_at",
        "acknowledgements",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `message_reactions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MessageReaction {
    /// VARCHAR(36) NOT NULL
    pub message_id: String,
    /// VARCHAR(10) NOT NULL
    pub message_source: String,
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(10) NOT NULL
    pub emoji: String,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl MessageReaction {
    pub const TABLE: &'static str = "message_reactions";
    pub const COLUMNS: &'static [&'static str] = &[
        "message_id",
        "message_source",
        "user_id",
        "emoji",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `video_rooms`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VideoRoom {
    /// VARCHAR(64) NOT NULL
    pub room_id: String,
    /// VARCHAR(255)
    pub name: Option<String>,
    /// UUID NOT NULL
    pub created_by_id: uuid::Uuid,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub project_id: Option<uuid::Uuid>,
    /// BOOLEAN
    pub is_active: Option<bool>,
    /// INTEGER
    pub max_participants: Option<i32>,
    /// TIMESTAMP WITH TIME ZONE
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB
    pub participants: Option<serde_json::Value>,
    /// JSONB
    pub waiting_participants: Option<serde_json::Value>,
    /// VARCHAR(64)
    pub guest_token_secret: Option<String>,
    /// JSONB
    pub invited_user_ids: Option<serde_json::Value>,
    /// VARCHAR(500)
    pub recording_key: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub recording_started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub recording_ended_at: Option<chrono::DateTime<chrono::Utc>>,
    /// INTEGER
    pub recording_size_bytes: Option<i32>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl VideoRoom {
    pub const TABLE: &'static str = "video_rooms";
    pub const COLUMNS: &'static [&'static str] = &[
        "room_id",
        "name",
        "created_by_id",
        "deal_id",
        "project_id",
        "is_active",
        "max_participants",
        "started_at",
        "ended_at",
        "participants",
        "waiting_participants",
        "guest_token_secret",
        "invited_user_ids",
        "recording_key",
        "recording_started_at",
        "recording_ended_at",
        "recording_size_bytes",
        "id",
        "created_at",
        "updated_at",
    ];
}
