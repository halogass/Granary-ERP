//! Migration runner.
//!
//! Applies `crates/db/migrations/NNNN_name.sql` in order (the list is in `revisions.rs`). `0001_baseline.sql`
//! creates the whole schema and its default data; later files change it. Applied migrations are recorded in
//! `schema_migrations` with a checksum of their SQL; a file that changed after it ran is logged as a warning.
//!
//! Pending migrations run in ONE transaction: if any fails, the whole upgrade rolls back and the database stays
//! where it was. A session-level advisory lock serialises runners.

mod revisions;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, Row};
use std::time::Instant;

/// One migration.
#[derive(Debug, Clone, Copy)]
pub struct MigrationDef {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

impl MigrationDef {
    pub fn checksum(&self) -> String {
        hex::encode(Sha256::digest(self.sql.as_bytes()))
    }
}

/// Every migration, in order.
pub fn migrations() -> &'static [MigrationDef] {
    revisions::MIGRATIONS
}

/// What a `migrate` call did.
#[derive(Debug, Default, Clone)]
pub struct MigrateReport {
    /// Migrations whose SQL ran in this call.
    pub applied: Vec<i64>,
}

/// Applied state of one migration, for `status`.
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    pub version: i64,
    pub name: &'static str,
    pub applied: bool,
    pub checksum_mismatch: bool,
}

const LOCK_KEY: i64 = 0x4752_414e_4d49_4752; // "GRANMIGR"

const BOOKKEEPING_DDL: &str = "\
CREATE TABLE IF NOT EXISTS public.schema_migrations (
    version bigint PRIMARY KEY,
    name text NOT NULL,
    checksum text NOT NULL,
    execution_ms bigint NOT NULL DEFAULT 0,
    applied_at timestamptz NOT NULL DEFAULT now()
)";

/// Apply every pending migration.
pub async fn migrate(pool: &PgPool) -> Result<MigrateReport> {
    migrate_to(pool, None).await
}

/// Apply pending migrations up to and including `target` (all when `None`).
pub async fn migrate_to(pool: &PgPool, target: Option<i64>) -> Result<MigrateReport> {
    let mut conn = pool
        .acquire()
        .await
        .context("acquire connection for migrations")?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    let result = run_locked(&mut conn, target).await;
    let unlock = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(LOCK_KEY)
        .execute(&mut *conn)
        .await;
    let report = result?;
    unlock?;
    Ok(report)
}

async fn run_locked(conn: &mut PgConnection, target: Option<i64>) -> Result<MigrateReport> {
    let mut report = MigrateReport::default();
    sqlx::raw_sql(BOOKKEEPING_DDL).execute(&mut *conn).await?;
    let applied = applied_versions(conn).await?;
    for (version, checksum) in &applied {
        if let Some(m) = migrations().iter().find(|m| m.version == *version) {
            if &m.checksum() != checksum {
                tracing::warn!(
                    version,
                    name = m.name,
                    "applied migration file changed since it ran"
                );
            }
        }
    }

    let mut tx = conn.begin().await?;
    for m in migrations() {
        if target.is_some_and(|t| m.version > t) {
            break;
        }
        if applied.iter().any(|(v, _)| *v == m.version) {
            continue;
        }
        let started = Instant::now();
        sqlx::raw_sql(m.sql)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("migration {:04}_{} failed", m.version, m.name))?;
        sqlx::query(
            "INSERT INTO public.schema_migrations (version, name, checksum, execution_ms) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(m.version)
        .bind(m.name)
        .bind(m.checksum())
        .bind(started.elapsed().as_millis() as i64)
        .execute(&mut *tx)
        .await?;
        tracing::info!(version = m.version, name = m.name, "applied migration");
        report.applied.push(m.version);
    }
    tx.commit().await?;
    Ok(report)
}

/// Applied/pending state of every migration.
pub async fn status(pool: &PgPool) -> Result<Vec<MigrationStatus>> {
    let mut conn = pool.acquire().await?;
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public.schema_migrations') IS NOT NULL")
            .fetch_one(&mut *conn)
            .await?;
    let applied = if exists {
        applied_versions(&mut conn).await?
    } else {
        Vec::new()
    };
    Ok(migrations()
        .iter()
        .map(|m| {
            let row = applied.iter().find(|(v, _)| *v == m.version);
            MigrationStatus {
                version: m.version,
                name: m.name,
                applied: row.is_some(),
                checksum_mismatch: row.is_some_and(|r| r.1 != m.checksum()),
            }
        })
        .collect())
}

async fn applied_versions(conn: &mut PgConnection) -> Result<Vec<(i64, String)>> {
    let rows =
        sqlx::query("SELECT version, checksum FROM public.schema_migrations ORDER BY version")
            .fetch_all(&mut *conn)
            .await?;
    rows.iter()
        .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_is_contiguous() {
        for (i, m) in migrations().iter().enumerate() {
            assert_eq!(m.version, i as i64 + 1);
            assert!(!m.sql.trim().is_empty(), "{:04} is empty", m.version);
        }
        assert_eq!(migrations()[0].name, "baseline");
    }
}
