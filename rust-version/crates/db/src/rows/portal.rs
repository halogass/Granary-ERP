// Row structs of the portal tables: one struct per table, its columns in order.

/// Row of `portal_registration_requests`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PortalRegistrationRequest {
    /// VARCHAR(20) NOT NULL
    pub portal_type: String,
    /// VARCHAR(20) NOT NULL
    pub status: String,
    /// VARCHAR(255) NOT NULL
    pub company_name: String,
    /// VARCHAR(255) NOT NULL
    pub contact_name: String,
    /// VARCHAR(255) NOT NULL
    pub email: String,
    /// VARCHAR(50)
    pub phone: Option<String>,
    /// VARCHAR(50)
    pub vendor_type: Option<String>,
    /// VARCHAR(50)
    pub partner_type: Option<String>,
    /// VARCHAR(100)
    pub country: Option<String>,
    /// VARCHAR(500)
    pub website: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub reviewed_by_id: Option<uuid::Uuid>,
    /// TIMESTAMP WITH TIME ZONE
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TEXT
    pub rejection_reason: Option<String>,
    /// UUID
    pub created_user_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PortalRegistrationRequest {
    pub const TABLE: &'static str = "portal_registration_requests";
    pub const COLUMNS: &'static [&'static str] = &[
        "portal_type",
        "status",
        "company_name",
        "contact_name",
        "email",
        "phone",
        "vendor_type",
        "partner_type",
        "country",
        "website",
        "notes",
        "reviewed_by_id",
        "reviewed_at",
        "rejection_reason",
        "created_user_id",
        "id",
        "created_at",
        "updated_at",
    ];
}
