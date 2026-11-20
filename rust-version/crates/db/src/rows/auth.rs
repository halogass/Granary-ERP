// Row structs of the auth tables: one struct per table, its columns in order.

/// Row of `mcp_oauth_clients`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpOAuthClient {
    /// VARCHAR(64) NOT NULL
    pub client_id: String,
    /// VARCHAR(200) NOT NULL
    pub client_name: String,
    /// JSONB NOT NULL
    pub redirect_uris: serde_json::Value,
    /// TIMESTAMP WITH TIME ZONE
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl McpOAuthClient {
    pub const TABLE: &'static str = "mcp_oauth_clients";
    pub const COLUMNS: &'static [&'static str] = &[
        "client_id",
        "client_name",
        "redirect_uris",
        "last_used_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `mcp_oauth_codes`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpOAuthCode {
    /// VARCHAR(64) NOT NULL
    pub code_hash: String,
    /// VARCHAR(64) NOT NULL
    pub client_id: String,
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// TEXT NOT NULL
    pub redirect_uri: String,
    /// VARCHAR(128) NOT NULL
    pub code_challenge: String,
    /// JSONB NOT NULL
    pub scopes: serde_json::Value,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl McpOAuthCode {
    pub const TABLE: &'static str = "mcp_oauth_codes";
    pub const COLUMNS: &'static [&'static str] = &[
        "code_hash",
        "client_id",
        "user_id",
        "redirect_uri",
        "code_challenge",
        "scopes",
        "expires_at",
        "used_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `permissions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Permission {
    /// VARCHAR(100) NOT NULL
    pub code: String,
    /// VARCHAR(200) NOT NULL
    pub name: String,
    /// VARCHAR(50) NOT NULL
    pub module: String,
    /// TEXT
    pub description: Option<String>,
    /// BOOLEAN
    pub is_system_permission: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Permission {
    pub const TABLE: &'static str = "permissions";
    pub const COLUMNS: &'static [&'static str] = &[
        "code",
        "name",
        "module",
        "description",
        "is_system_permission",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `personal_access_tokens`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersonalAccessToken {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(120) NOT NULL
    pub name: String,
    /// VARCHAR(16) NOT NULL
    pub token_prefix: String,
    /// VARCHAR(255) NOT NULL
    pub token_hash: String,
    /// JSONB
    pub scopes: Option<serde_json::Value>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// VARCHAR(10) NOT NULL
    pub kind: String,
    /// VARCHAR(64)
    pub client_id: Option<String>,
    /// VARCHAR(64)
    pub refresh_token_hash: Option<String>,
    /// TIMESTAMP WITH TIME ZONE
    pub refresh_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PersonalAccessToken {
    pub const TABLE: &'static str = "personal_access_tokens";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "name",
        "token_prefix",
        "token_hash",
        "scopes",
        "last_used_at",
        "expires_at",
        "revoked_at",
        "kind",
        "client_id",
        "refresh_token_hash",
        "refresh_expires_at",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `role_permissions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RolePermissions {
    /// UUID NOT NULL
    pub role_id: uuid::Uuid,
    /// UUID NOT NULL
    pub permission_id: uuid::Uuid,
}

impl RolePermissions {
    pub const TABLE: &'static str = "role_permissions";
    pub const COLUMNS: &'static [&'static str] = &["role_id", "permission_id"];
}

/// Row of `roles`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Role {
    /// VARCHAR(100) NOT NULL
    pub name: String,
    /// VARCHAR(100) NOT NULL
    pub slug: String,
    /// TEXT
    pub description: Option<String>,
    /// BOOLEAN
    pub is_system_role: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Role {
    pub const TABLE: &'static str = "roles";
    pub const COLUMNS: &'static [&'static str] = &[
        "name",
        "slug",
        "description",
        "is_system_role",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `user_permissions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserPermissions {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// UUID NOT NULL
    pub permission_id: uuid::Uuid,
    /// BOOLEAN
    pub is_deny: Option<bool>,
}

impl UserPermissions {
    pub const TABLE: &'static str = "user_permissions";
    pub const COLUMNS: &'static [&'static str] = &["user_id", "permission_id", "is_deny"];
}

/// Row of `user_roles`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserRoles {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// UUID NOT NULL
    pub role_id: uuid::Uuid,
}

impl UserRoles {
    pub const TABLE: &'static str = "user_roles";
    pub const COLUMNS: &'static [&'static str] = &["user_id", "role_id"];
}

/// Row of `user_sessions`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserSession {
    /// UUID NOT NULL
    pub user_id: uuid::Uuid,
    /// VARCHAR(500) NOT NULL
    pub token_hash: String,
    /// VARCHAR(20)
    pub token_type: Option<String>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// VARCHAR(45)
    pub ip_address: Option<String>,
    /// TEXT
    pub user_agent: Option<String>,
    /// BOOLEAN
    pub is_revoked: Option<bool>,
    /// TIMESTAMP WITH TIME ZONE
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// UUID: the login this row belongs to (the first row's id), kept across refreshes (stage 8)
    pub family_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl UserSession {
    pub const TABLE: &'static str = "user_sessions";
    pub const COLUMNS: &'static [&'static str] = &[
        "user_id",
        "token_hash",
        "token_type",
        "expires_at",
        "ip_address",
        "user_agent",
        "is_revoked",
        "revoked_at",
        "family_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `users`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct User {
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// VARCHAR(255) NOT NULL
    pub name: String,
    /// VARCHAR(255) NOT NULL
    pub password_hash: String,
    /// VARCHAR(50)
    pub phone: Option<String>,
    /// VARCHAR(500)
    pub avatar_url: Option<String>,
    /// TEXT
    pub signature_data: Option<String>,
    /// JSONB
    pub email_signature_fields: Option<serde_json::Value>,
    /// BOOLEAN
    pub email_signature_enabled: Option<bool>,
    /// VARCHAR(60)
    pub wa_signature_name: Option<String>,
    /// VARCHAR(300)
    pub wa_signature_custom: Option<String>,
    /// VARCHAR(100)
    pub department: Option<String>,
    /// VARCHAR(100)
    pub position: Option<String>,
    /// BOOLEAN NOT NULL
    pub is_active: bool,
    /// BOOLEAN NOT NULL
    pub is_superadmin: bool,
    /// BOOLEAN NOT NULL
    pub is_system_admin: bool,
    /// VARCHAR(20) NOT NULL
    pub portal_type: String,
    /// UUID
    pub partner_account_id: Option<uuid::Uuid>,
    /// UUID
    pub vendor_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TIMESTAMP WITH TIME ZONE
    pub password_changed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BOOLEAN
    pub must_change_password: Option<bool>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl User {
    pub const TABLE: &'static str = "users";
    pub const COLUMNS: &'static [&'static str] = &[
        "email",
        "name",
        "password_hash",
        "phone",
        "avatar_url",
        "signature_data",
        "email_signature_fields",
        "email_signature_enabled",
        "wa_signature_name",
        "wa_signature_custom",
        "department",
        "position",
        "is_active",
        "is_superadmin",
        "is_system_admin",
        "portal_type",
        "partner_account_id",
        "vendor_id",
        "last_login_at",
        "password_changed_at",
        "must_change_password",
        "id",
        "created_at",
        "updated_at",
    ];
}
