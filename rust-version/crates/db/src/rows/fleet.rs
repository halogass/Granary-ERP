// Row structs of the fleet tables: one struct per table, its columns in order.

/// Row of `fleet_assignments`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FleetAssignment {
    /// UUID NOT NULL
    pub vehicle_id: uuid::Uuid,
    /// UUID
    pub employee_id: Option<uuid::Uuid>,
    /// DATE NOT NULL
    pub start_date: chrono::NaiveDate,
    /// DATE
    pub end_date: Option<chrono::NaiveDate>,
    /// VARCHAR(255)
    pub purpose: Option<String>,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub assigned_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl FleetAssignment {
    pub const TABLE: &'static str = "fleet_assignments";
    pub const COLUMNS: &'static [&'static str] = &[
        "vehicle_id",
        "employee_id",
        "start_date",
        "end_date",
        "purpose",
        "notes",
        "assigned_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `fleet_fuel_logs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FleetFuelLog {
    /// UUID NOT NULL
    pub vehicle_id: uuid::Uuid,
    /// DATE NOT NULL
    pub log_date: chrono::NaiveDate,
    /// NUMERIC(10, 1)
    pub odometer_reading: Option<rust_decimal::Decimal>,
    /// NUMERIC(8, 2) NOT NULL
    pub fuel_liters: rust_decimal::Decimal,
    /// NUMERIC(14, 2) NOT NULL
    pub fuel_cost: rust_decimal::Decimal,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// VARCHAR(255)
    pub fuel_station: Option<String>,
    /// UUID
    pub logged_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl FleetFuelLog {
    pub const TABLE: &'static str = "fleet_fuel_logs";
    pub const COLUMNS: &'static [&'static str] = &[
        "vehicle_id",
        "log_date",
        "odometer_reading",
        "fuel_liters",
        "fuel_cost",
        "currency",
        "fuel_station",
        "logged_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `fleet_odometer_logs`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FleetOdometerLog {
    /// UUID NOT NULL
    pub vehicle_id: uuid::Uuid,
    /// DATE NOT NULL
    pub log_date: chrono::NaiveDate,
    /// NUMERIC(10, 1) NOT NULL
    pub odometer_reading: rust_decimal::Decimal,
    /// TEXT
    pub notes: Option<String>,
    /// UUID
    pub logged_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl FleetOdometerLog {
    pub const TABLE: &'static str = "fleet_odometer_logs";
    pub const COLUMNS: &'static [&'static str] = &[
        "vehicle_id",
        "log_date",
        "odometer_reading",
        "notes",
        "logged_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `fleet_service_records`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FleetServiceRecord {
    /// UUID NOT NULL
    pub vehicle_id: uuid::Uuid,
    /// DATE NOT NULL
    pub service_date: chrono::NaiveDate,
    /// VARCHAR(30) NOT NULL
    pub service_type: String,
    /// TEXT
    pub description: Option<String>,
    /// NUMERIC(14, 2)
    pub cost: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// VARCHAR(255)
    pub vendor_name: Option<String>,
    /// NUMERIC(10, 1)
    pub odometer_reading: Option<rust_decimal::Decimal>,
    /// DATE
    pub next_service_date: Option<chrono::NaiveDate>,
    /// NUMERIC(10, 1)
    pub next_service_odometer: Option<rust_decimal::Decimal>,
    /// UUID
    pub logged_by_id: Option<uuid::Uuid>,
    /// UUID NOT NULL
    pub id: uuid::Uuid,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// TIMESTAMP WITH TIME ZONE NOT NULL
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl FleetServiceRecord {
    pub const TABLE: &'static str = "fleet_service_records";
    pub const COLUMNS: &'static [&'static str] = &[
        "vehicle_id",
        "service_date",
        "service_type",
        "description",
        "cost",
        "currency",
        "vendor_name",
        "odometer_reading",
        "next_service_date",
        "next_service_odometer",
        "logged_by_id",
        "id",
        "created_at",
        "updated_at",
    ];
}

/// Row of `fleet_vehicles`.
#[derive(sqlx::FromRow, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FleetVehicle {
    /// VARCHAR(20) NOT NULL
    pub plate_number: String,
    /// VARCHAR(100) NOT NULL
    pub make: String,
    /// VARCHAR(100) NOT NULL
    pub model: String,
    /// INTEGER NOT NULL
    pub year: i32,
    /// VARCHAR(50)
    pub color: Option<String>,
    /// VARCHAR(30)
    pub vehicle_type: Option<String>,
    /// VARCHAR(20)
    pub fuel_type: Option<String>,
    /// VARCHAR(20)
    pub status: Option<String>,
    /// DATE
    pub purchase_date: Option<chrono::NaiveDate>,
    /// NUMERIC(18, 2)
    pub purchase_price: Option<rust_decimal::Decimal>,
    /// VARCHAR(3)
    pub currency: Option<String>,
    /// VARCHAR(100)
    pub insurance_policy: Option<String>,
    /// DATE
    pub insurance_expiry: Option<chrono::NaiveDate>,
    /// DATE
    pub registration_expiry: Option<chrono::NaiveDate>,
    /// NUMERIC(10, 1)
    pub odometer_current: Option<rust_decimal::Decimal>,
    /// TEXT
    pub notes: Option<String>,
    /// VARCHAR(500)
    pub image_url: Option<String>,
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

impl FleetVehicle {
    pub const TABLE: &'static str = "fleet_vehicles";
    pub const COLUMNS: &'static [&'static str] = &[
        "plate_number",
        "make",
        "model",
        "year",
        "color",
        "vehicle_type",
        "fuel_type",
        "status",
        "purchase_date",
        "purchase_price",
        "currency",
        "insurance_policy",
        "insurance_expiry",
        "registration_expiry",
        "odometer_current",
        "notes",
        "image_url",
        "id",
        "created_at",
        "updated_at",
        "is_deleted",
        "deleted_at",
    ];
}
