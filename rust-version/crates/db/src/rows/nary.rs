// Row structs of the nary tables: one struct per table, its columns in order.

/// Row of `agent_pending_actions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentPendingAction {
    /// VARCHAR(60) NOT NULL
    pub action: String,
    /// VARCHAR(20) NOT NULL
    pub origin: String,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// INTEGER NOT NULL
    pub level: i32,
    /// BOOLEAN NOT NULL
    pub customer_facing: bool,
    /// JSONB NOT NULL
    pub payload: serde_json::Value,
    /// JSONB
    pub edited_payload: Option<serde_json::Value>,
    /// TEXT
    pub rationale: Option<String>,
    /// VARCHAR(40)
    pub sop_ref: Option<String>,
    /// JSONB
    pub sources: Option<serde_json::Value>,
    /// JSONB
    pub diff: Option<serde_json::Value>,
    /// JSONB
    pub price: Option<serde_json::Value>,
    /// VARCHAR(40)
    pub entity_type: Option<String>,
    /// UUID
    pub entity_id: Option<uuid::Uuid>,
    /// UUID
    pub run_id: Option<uuid::Uuid>,
    /// UUID
    pub actor_id: Option<uuid::Uuid>,
    /// UUID
    pub assignee_id: Option<uuid::Uuid>,
    /// UUID
    pub decided_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub reject_reason: Option<String>,
    /// NUMERIC(6, 2)
    pub edit_ratio: Option<rust_decimal::Decimal>,
    /// JSONB
    pub result: Option<serde_json::Value>,
    /// TEXT
    pub error: Option<String>,
    /// TEXT
    pub actual_text: Option<String>,
    /// NUMERIC(6, 2)
    pub diff_ratio: Option<rust_decimal::Decimal>,
    /// TIMESTAMP WITH TIME ZONE
    pub compared_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AgentPendingAction {
    pub const TABLE: &'static str = "agent_pending_actions";
    pub const COLUMNS: &'static [&'static str] = &[
        "action",
        "origin",
        "status",
        "level",
        "customer_facing",
        "payload",
        "edited_payload",
        "rationale",
        "sop_ref",
        "sources",
        "diff",
        "price",
        "entity_type",
        "entity_id",
        "run_id",
        "actor_id",
        "assignee_id",
        "decided_by_id",
        "decided_at",
        "reject_reason",
        "edit_ratio",
        "result",
        "error",
        "actual_text",
        "diff_ratio",
        "compared_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `agent_runs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentRun {
    /// VARCHAR(20) NOT NULL
    pub trigger_type: String,
    /// VARCHAR(80)
    pub trigger_ref: Option<String>,
    /// VARCHAR(60) NOT NULL
    pub job: String,
    /// VARCHAR(80) NOT NULL
    pub context: String,
    /// VARCHAR(40)
    pub sop_ref: Option<String>,
    /// VARCHAR(80)
    pub model: Option<String>,
    /// INTEGER NOT NULL
    pub prompt_tokens: i32,
    /// INTEGER NOT NULL
    pub completion_tokens: i32,
    /// INTEGER NOT NULL
    pub cache_read_tokens: i32,
    /// INTEGER NOT NULL
    pub cache_write_tokens: i32,
    /// NUMERIC(12, 6) NOT NULL
    pub est_cost: rust_decimal::Decimal,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// TEXT
    pub outcome: Option<String>,
    /// TEXT
    pub error: Option<String>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AgentRun {
    pub const TABLE: &'static str = "agent_runs";
    pub const COLUMNS: &'static [&'static str] = &[
        "trigger_type",
        "trigger_ref",
        "job",
        "context",
        "sop_ref",
        "model",
        "prompt_tokens",
        "completion_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "est_cost",
        "status",
        "outcome",
        "error",
        "started_at",
        "finished_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `agent_steps`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentStep {
    /// UUID NOT NULL
    pub run_id: uuid::Uuid,
    /// INTEGER NOT NULL
    pub seq: i32,
    /// VARCHAR(20) NOT NULL
    pub kind: String,
    /// VARCHAR(80)
    pub name: Option<String>,
    /// JSONB
    pub input: Option<serde_json::Value>,
    /// JSONB
    pub output: Option<serde_json::Value>,
    /// INTEGER NOT NULL
    pub prompt_tokens: i32,
    /// INTEGER NOT NULL
    pub completion_tokens: i32,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AgentStep {
    pub const TABLE: &'static str = "agent_steps";
    pub const COLUMNS: &'static [&'static str] = &[
        "run_id",
        "seq",
        "kind",
        "name",
        "input",
        "output",
        "prompt_tokens",
        "completion_tokens",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `domain_events`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DomainEvent {
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    /// VARCHAR(60) NOT NULL
    pub type_: String,
    /// VARCHAR(40)
    pub entity_type: Option<String>,
    /// UUID
    pub entity_id: Option<uuid::Uuid>,
    /// JSONB NOT NULL
    pub payload: serde_json::Value,
    /// UUID
    pub actor_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub dispatched_at: Option<chrono::DateTime<chrono::Utc>>,
    /// INTEGER NOT NULL
    pub attempts: i32,
    /// TEXT
    pub last_error: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl DomainEvent {
    pub const TABLE: &'static str = "domain_events";
    pub const COLUMNS: &'static [&'static str] = &[
        "type",
        "entity_type",
        "entity_id",
        "payload",
        "actor_id",
        "occurred_at",
        "dispatched_at",
        "attempts",
        "last_error",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `google_accounts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoogleAccount {
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// TEXT
    pub access_token: Option<String>,
    /// TEXT
    pub refresh_token: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub scopes: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// TEXT
    pub last_error: Option<String>,
    /// UUID
    pub connected_by_id: Option<uuid::Uuid>,
    /// VARCHAR(64)
    pub oauth_state: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl GoogleAccount {
    pub const TABLE: &'static str = "google_accounts";
    pub const COLUMNS: &'static [&'static str] = &[
        "email",
        "access_token",
        "refresh_token",
        "expires_at",
        "scopes",
        "status",
        "last_error",
        "connected_by_id",
        "oauth_state",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_autonomy`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryAutonomy {
    /// VARCHAR(60) NOT NULL
    pub action: String,
    /// INTEGER NOT NULL
    pub streak: i32,
    /// TIMESTAMP WITH TIME ZONE
    pub candidate_since: Option<chrono::DateTime<chrono::Utc>>,
    /// INTEGER NOT NULL
    pub approvals_total: i32,
    /// INTEGER NOT NULL
    pub edits_total: i32,
    /// INTEGER NOT NULL
    pub rejects_total: i32,
    /// TIMESTAMP WITH TIME ZONE
    pub last_changed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(120)
    pub last_change: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryAutonomy {
    pub const TABLE: &'static str = "nary_autonomy";
    pub const COLUMNS: &'static [&'static str] = &[
        "action",
        "streak",
        "candidate_since",
        "approvals_total",
        "edits_total",
        "rejects_total",
        "last_changed_at",
        "last_change",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_cadences`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryCadence {
    /// UUID NOT NULL
    pub quotation_id: uuid::Uuid,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// INTEGER NOT NULL
    pub day: i32,
    /// VARCHAR(10) NOT NULL
    pub channel: String,
    /// TEXT
    pub intent: Option<String>,
    /// VARCHAR(30)
    pub topic: Option<String>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub due_at: chrono::DateTime<chrono::Utc>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// TEXT
    pub reason: Option<String>,
    /// UUID
    pub card_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub done_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// TEXT (migration 0133: the drafted text, kept so a rerun never drafts again)
    pub draft_text: Option<String>,
    /// UUID (migration 0133: the step whose message carried this one)
    pub merged_into: Option<uuid::Uuid>,
}

impl NaryCadence {
    pub const TABLE: &'static str = "nary_cadences";
    pub const COLUMNS: &'static [&'static str] = &[
        "quotation_id",
        "deal_id",
        "contact_id",
        "day",
        "channel",
        "intent",
        "topic",
        "due_at",
        "status",
        "reason",
        "card_id",
        "done_at",
        "id",
        "created_at",
        "updated_at",
        "draft_text",
        "merged_into",
    ];
}

/// Row of `nary_owner_topics`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryOwnerTopic {
    /// VARCHAR(30) NOT NULL
    pub kind: String,
    /// VARCHAR(200) NOT NULL
    pub tag: String,
    /// TEXT NOT NULL
    pub text: String,
    /// VARCHAR(40)
    pub ref_type: Option<String>,
    /// UUID
    pub ref_id: Option<uuid::Uuid>,
    /// JSONB
    pub facts: Option<serde_json::Value>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// BOOLEAN NOT NULL
    pub urgent: bool,
    /// BOOLEAN NOT NULL
    pub anytime: bool,
    /// JSONB
    pub numbered: Option<serde_json::Value>,
    /// JSONB
    pub resume: Option<serde_json::Value>,
    /// TIMESTAMP WITH TIME ZONE
    pub send_after: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB
    pub wa_message_keys: Option<serde_json::Value>,
    /// UUID
    pub conversation_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub remind_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub reminded_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub answered_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub answer: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryOwnerTopic {
    pub const TABLE: &'static str = "nary_owner_topics";
    pub const COLUMNS: &'static [&'static str] = &[
        "kind",
        "tag",
        "text",
        "ref_type",
        "ref_id",
        "facts",
        "status",
        "urgent",
        "anytime",
        "numbered",
        "resume",
        "send_after",
        "sent_at",
        "wa_message_keys",
        "conversation_id",
        "remind_at",
        "reminded_at",
        "answered_at",
        "answer",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_expected_arrivals`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryExpectedArrival {
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// VARCHAR(32)
    pub phone: Option<String>,
    /// VARCHAR(255)
    pub email: Option<String>,
    /// VARCHAR(200)
    pub name: Option<String>,
    /// VARCHAR(255)
    pub company: Option<String>,
    /// TEXT
    pub message: Option<String>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub due_at: chrono::DateTime<chrono::Utc>,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// VARCHAR(120)
    pub chat_id: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub arrived_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub notified_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryExpectedArrival {
    pub const TABLE: &'static str = "nary_expected_arrivals";
    pub const COLUMNS: &'static [&'static str] = &[
        "lead_id",
        "phone",
        "email",
        "name",
        "company",
        "message",
        "due_at",
        "status",
        "chat_id",
        "arrived_at",
        "notified_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_identity_links`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryIdentityLink {
    /// VARCHAR(20) NOT NULL
    pub subject_type: String,
    /// VARCHAR(120) NOT NULL
    pub subject_ref: String,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// VARCHAR(10) NOT NULL
    pub tier: String,
    /// INTEGER NOT NULL
    pub score: i32,
    /// JSONB NOT NULL
    pub signals: serde_json::Value,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// BOOLEAN NOT NULL
    pub cascaded: bool,
    /// TEXT
    pub review: Option<String>,
    /// JSONB
    pub previous: Option<serde_json::Value>,
    /// UUID
    pub decided_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryIdentityLink {
    pub const TABLE: &'static str = "nary_identity_links";
    pub const COLUMNS: &'static [&'static str] = &[
        "subject_type",
        "subject_ref",
        "contact_id",
        "lead_id",
        "deal_id",
        "tier",
        "score",
        "signals",
        "status",
        "cascaded",
        "review",
        "previous",
        "decided_by_id",
        "decided_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_insights`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryInsight {
    /// VARCHAR(20) NOT NULL
    pub kind: String,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub meeting_id: Option<uuid::Uuid>,
    /// JSONB NOT NULL
    pub content: serde_json::Value,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// VARCHAR(80)
    pub model: Option<String>,
    /// UUID
    pub run_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryInsight {
    pub const TABLE: &'static str = "nary_insights";
    pub const COLUMNS: &'static [&'static str] = &[
        "kind",
        "deal_id",
        "lead_id",
        "meeting_id",
        "content",
        "status",
        "model",
        "run_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_meetings`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryMeeting {
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// VARCHAR(120)
    pub chat_id: Option<String>,
    /// VARCHAR(10) NOT NULL
    pub channel: String,
    /// VARCHAR(5)
    pub language: Option<String>,
    /// VARCHAR(60)
    pub customer_tz: Option<String>,
    /// VARCHAR(2)
    pub country: Option<String>,
    /// JSONB NOT NULL
    pub slots: serde_json::Value,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// INTEGER
    pub picked: Option<i32>,
    /// TIMESTAMP WITH TIME ZONE
    pub starts_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub ends_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(255)
    pub event_id: Option<String>,
    /// VARCHAR(500)
    pub meet_link: Option<String>,
    /// VARCHAR(255)
    pub attendee_email: Option<String>,
    /// VARCHAR(255)
    pub title: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub day_reminder_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub hour_reminder_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub brief_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub card_id: Option<uuid::Uuid>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(10) NOT NULL
    pub origin: String,
    /// UUID
    pub booked_by_id: Option<uuid::Uuid>,
    /// JSONB NOT NULL
    pub attendees: serde_json::Value,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryMeeting {
    pub const TABLE: &'static str = "nary_meetings";
    pub const COLUMNS: &'static [&'static str] = &[
        "deal_id",
        "lead_id",
        "contact_id",
        "chat_id",
        "channel",
        "language",
        "customer_tz",
        "country",
        "slots",
        "status",
        "picked",
        "starts_at",
        "ends_at",
        "event_id",
        "meet_link",
        "attendee_email",
        "title",
        "day_reminder_at",
        "hour_reminder_at",
        "brief_at",
        "card_id",
        "notes",
        "origin",
        "booked_by_id",
        "attendees",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_memory`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryMemory {
    /// VARCHAR(20) NOT NULL
    pub layer: String,
    /// VARCHAR(30)
    pub subject_type: Option<String>,
    /// VARCHAR(64)
    pub subject_id: Option<String>,
    /// VARCHAR(200)
    pub title: Option<String>,
    /// TEXT NOT NULL
    pub content: String,
    /// VARCHAR(30) NOT NULL
    pub source_type: String,
    /// VARCHAR(200)
    pub source_ref: Option<String>,
    /// UUID
    pub author_id: Option<uuid::Uuid>,
    /// BOOLEAN NOT NULL
    pub authored_by_nary: bool,
    /// BOOLEAN NOT NULL
    pub trusted: bool,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// UUID
    pub approved_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryMemory {
    pub const TABLE: &'static str = "nary_memory";
    pub const COLUMNS: &'static [&'static str] = &[
        "layer",
        "subject_type",
        "subject_id",
        "title",
        "content",
        "source_type",
        "source_ref",
        "author_id",
        "authored_by_nary",
        "trusted",
        "status",
        "approved_by_id",
        "approved_at",
        "expires_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_reply_alerts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryReplyAlert {
    /// VARCHAR(10) NOT NULL
    pub channel: String,
    /// VARCHAR(120)
    pub chat_id: Option<String>,
    /// UUID
    pub thread_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub message_ref: Option<String>,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub who: Option<String>,
    /// TEXT NOT NULL
    pub summary: String,
    /// BOOLEAN NOT NULL
    pub needs_action: bool,
    /// TEXT
    pub action: Option<String>,
    /// UUID
    pub card_id: Option<uuid::Uuid>,
    /// UUID
    pub topic_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub briefed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryReplyAlert {
    pub const TABLE: &'static str = "nary_reply_alerts";
    pub const COLUMNS: &'static [&'static str] = &[
        "channel",
        "chat_id",
        "thread_id",
        "message_ref",
        "contact_id",
        "lead_id",
        "deal_id",
        "who",
        "summary",
        "needs_action",
        "action",
        "card_id",
        "topic_id",
        "briefed_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_role_assignments`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryRoleAssignment {
    /// VARCHAR(40) NOT NULL
    pub sop_role: String,
    /// BOOLEAN NOT NULL
    pub is_nary: bool,
    /// UUID
    pub user_id: Option<uuid::Uuid>,
    /// VARCHAR(255)
    pub note: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryRoleAssignment {
    pub const TABLE: &'static str = "nary_role_assignments";
    pub const COLUMNS: &'static [&'static str] = &[
        "sop_role",
        "is_nary",
        "user_id",
        "note",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_scores`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryScore {
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// VARCHAR(120)
    pub chat_id: Option<String>,
    /// INTEGER NOT NULL
    pub budget: i32,
    /// INTEGER NOT NULL
    pub authority: i32,
    /// INTEGER NOT NULL
    pub need: i32,
    /// INTEGER NOT NULL
    pub timeline: i32,
    /// INTEGER NOT NULL
    pub total: i32,
    /// VARCHAR(10) NOT NULL
    pub class: String,
    /// JSONB NOT NULL
    pub red_flags: serde_json::Value,
    /// JSONB NOT NULL
    pub green_flags: serde_json::Value,
    /// TEXT
    pub rationale: Option<String>,
    /// VARCHAR(20)
    pub intent: Option<String>,
    /// VARCHAR(40)
    pub product: Option<String>,
    /// VARCHAR(80)
    pub model: Option<String>,
    /// BOOLEAN NOT NULL
    pub cascaded: bool,
    /// UUID
    pub run_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryScore {
    pub const TABLE: &'static str = "nary_scores";
    pub const COLUMNS: &'static [&'static str] = &[
        "lead_id",
        "deal_id",
        "chat_id",
        "budget",
        "authority",
        "need",
        "timeline",
        "total",
        "class",
        "red_flags",
        "green_flags",
        "rationale",
        "intent",
        "product",
        "model",
        "cascaded",
        "run_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `nary_triage_verdicts`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NaryTriageVerdict {
    /// VARCHAR(20) NOT NULL
    pub subject_type: String,
    /// VARCHAR(120) NOT NULL
    pub subject_ref: String,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// VARCHAR(20) NOT NULL
    pub verdict: String,
    /// VARCHAR(5)
    pub language: Option<String>,
    /// TEXT
    pub reason: Option<String>,
    /// TEXT
    pub excerpt: Option<String>,
    /// VARCHAR(60)
    pub job: Option<String>,
    /// VARCHAR(80)
    pub model: Option<String>,
    /// BOOLEAN NOT NULL
    pub cascaded: bool,
    /// VARCHAR(200)
    pub cascade_reason: Option<String>,
    /// VARCHAR(20)
    pub corrected_to: Option<String>,
    /// UUID
    pub corrected_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub corrected_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub correction_note: Option<String>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl NaryTriageVerdict {
    pub const TABLE: &'static str = "nary_triage_verdicts";
    pub const COLUMNS: &'static [&'static str] = &[
        "subject_type",
        "subject_ref",
        "lead_id",
        "verdict",
        "language",
        "reason",
        "excerpt",
        "job",
        "model",
        "cascaded",
        "cascade_reason",
        "corrected_to",
        "corrected_by_id",
        "corrected_at",
        "correction_note",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `staff_whatsapp_links`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StaffWhatsAppLink {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(120) NOT NULL
    pub chat_id: String,
    /// VARCHAR(50)
    pub phone: Option<String>,
    /// VARCHAR(120)
    pub label: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_primary: bool,
    /// UUID
    pub created_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl StaffWhatsAppLink {
    pub const TABLE: &'static str = "staff_whatsapp_links";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "chat_id",
        "phone",
        "label",
        "is_primary",
        "created_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `wa_chats`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WaChat {
    /// VARCHAR(120) NOT NULL
    pub chat_id: String,
    /// VARCHAR(32)
    pub phone: Option<String>,
    /// VARCHAR(200)
    pub display_name: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_my_contact: bool,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// BOOLEAN
    pub customer_started: Option<bool>,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub staff_user_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub human_until: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub checkin_sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub acked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub ack_pending_since: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub first_inbound_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_inbound_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_outbound_at: Option<chrono::DateTime<chrono::Utc>>,
    /// JSONB
    pub business_profile: Option<serde_json::Value>,
    /// TIMESTAMP WITH TIME ZONE
    pub profile_fetched_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(20)
    pub triage: Option<String>,
    /// VARCHAR(5)
    pub language: Option<String>,
    /// BOOLEAN NOT NULL
    pub attribution_asked: bool,
    /// TIMESTAMP WITH TIME ZONE
    pub excluded_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID
    pub excluded_by_id: Option<uuid::Uuid>,
    /// VARCHAR(40)
    pub exclude_reason: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub released_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WaChat {
    pub const TABLE: &'static str = "wa_chats";
    pub const COLUMNS: &'static [&'static str] = &[
        "chat_id",
        "phone",
        "display_name",
        "is_my_contact",
        "status",
        "customer_started",
        "contact_id",
        "lead_id",
        "deal_id",
        "staff_user_id",
        "human_until",
        "checkin_sent_at",
        "acked_at",
        "ack_pending_since",
        "first_inbound_at",
        "last_inbound_at",
        "last_outbound_at",
        "business_profile",
        "profile_fetched_at",
        "triage",
        "language",
        "attribution_asked",
        "excluded_at",
        "excluded_by_id",
        "exclude_reason",
        "released_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `wa_import_chats`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WaImportChat {
    /// VARCHAR(120) NOT NULL
    pub chat_id: String,
    /// VARCHAR(200)
    pub name: Option<String>,
    /// INTEGER
    pub message_count: Option<i32>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(10) NOT NULL
    pub decision: String,
    /// UUID
    pub decided_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(20) NOT NULL
    pub import_status: String,
    /// VARCHAR(255)
    pub cursor_before: Option<String>,
    /// INTEGER NOT NULL
    pub imported_count: i32,
    /// TIMESTAMP WITH TIME ZONE
    pub oldest_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub error: Option<String>,
    /// VARCHAR(20)
    pub classification: Option<String>,
    /// VARCHAR(120)
    pub batch_id: Option<String>,
    /// VARCHAR(20) NOT NULL
    pub read_status: String,
    /// TEXT
    pub summary: Option<String>,
    /// JSONB
    pub outputs: Option<serde_json::Value>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WaImportChat {
    pub const TABLE: &'static str = "wa_import_chats";
    pub const COLUMNS: &'static [&'static str] = &[
        "chat_id",
        "name",
        "message_count",
        "last_message_at",
        "decision",
        "decided_by_id",
        "decided_at",
        "import_status",
        "cursor_before",
        "imported_count",
        "oldest_at",
        "error",
        "classification",
        "batch_id",
        "read_status",
        "summary",
        "outputs",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `wa_messages`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WaMessage {
    /// VARCHAR(128) NOT NULL
    pub message_key: String,
    /// VARCHAR(255)
    pub serialized_id: Option<String>,
    /// VARCHAR(120) NOT NULL
    pub chat_id: String,
    /// VARCHAR(10) NOT NULL
    pub direction: String,
    /// BOOLEAN NOT NULL
    pub from_me: bool,
    /// BOOLEAN NOT NULL
    pub sent_by_erp: bool,
    /// BOOLEAN NOT NULL
    pub sent_by_nary: bool,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    /// VARCHAR(30) NOT NULL
    pub type_: String,
    /// TEXT
    pub body: Option<String>,
    /// VARCHAR(128)
    pub quoted_key: Option<String>,
    /// INTEGER
    pub ack: Option<i32>,
    /// TIMESTAMP WITH TIME ZONE
    pub wa_ts: Option<chrono::DateTime<chrono::Utc>>,
    /// BOOLEAN NOT NULL
    pub has_media: bool,
    /// VARCHAR(300)
    pub media_key: Option<String>,
    /// VARCHAR(120)
    pub media_mime: Option<String>,
    /// VARCHAR(255)
    pub media_name: Option<String>,
    /// INTEGER
    pub media_size: Option<i32>,
    /// VARCHAR(20) NOT NULL
    pub media_status: String,
    /// TEXT
    pub media_description: Option<String>,
    /// TEXT
    pub transcript: Option<String>,
    /// JSONB
    pub location: Option<serde_json::Value>,
    /// UUID
    pub contact_id: Option<uuid::Uuid>,
    /// UUID
    pub lead_id: Option<uuid::Uuid>,
    /// UUID
    pub deal_id: Option<uuid::Uuid>,
    /// UUID
    pub activity_id: Option<uuid::Uuid>,
    /// VARCHAR(10) NOT NULL
    pub origin: String,
    /// TIMESTAMP WITH TIME ZONE
    pub processed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl WaMessage {
    pub const TABLE: &'static str = "wa_messages";
    pub const COLUMNS: &'static [&'static str] = &[
        "message_key",
        "serialized_id",
        "chat_id",
        "direction",
        "from_me",
        "sent_by_erp",
        "sent_by_nary",
        "type",
        "body",
        "quoted_key",
        "ack",
        "wa_ts",
        "has_media",
        "media_key",
        "media_mime",
        "media_name",
        "media_size",
        "media_status",
        "media_description",
        "transcript",
        "location",
        "contact_id",
        "lead_id",
        "deal_id",
        "activity_id",
        "origin",
        "processed_at",
        "id",
        "created_at",
        "updated_at",
    ];
}
