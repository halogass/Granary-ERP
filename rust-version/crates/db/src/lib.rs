//! granary-db: schema migrations and row types for the Granary database.
//!
//! - [`migrate`]: the migration runner (`crates/db/migrations/*.sql`).
//! - [`rows`]: one sqlx row struct per table, generated from backend/app/models by
//!   compat-tests/schema/gen_rows.py.

pub mod migrate;
pub mod rows;
pub mod seed;
pub mod vector;

pub use migrate::{migrate, migrate_to, MigrateReport};
pub use vector::Vector;

/// Accept the Python-style `postgresql+asyncpg://` URL as well as plain `postgres://`.
pub fn normalize_database_url(url: &str) -> String {
    for prefix in [
        "postgresql+asyncpg://",
        "postgresql+psycopg://",
        "postgresql+psycopg2://",
    ] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return format!("postgres://{rest}");
        }
    }
    url.to_string()
}
