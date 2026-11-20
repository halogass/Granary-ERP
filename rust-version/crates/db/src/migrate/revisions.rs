//! Every migration, in order. Add a new one as `migrations/000N_name.sql` and a line here.

use super::MigrationDef;

pub static MIGRATIONS: &[MigrationDef] = &[MigrationDef {
    version: 1,
    name: "baseline",
    sql: include_str!("../../migrations/0001_baseline.sql"),
}];
