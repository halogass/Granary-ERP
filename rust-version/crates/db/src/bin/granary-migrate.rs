//! granary-migrate: apply database migrations.
//!
//!   granary-migrate [--database-url URL] [--target N] [--status]
//!
//! The URL defaults to $DATABASE_URL.

use anyhow::{bail, Context, Result};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,sqlx::postgres::notice=warn")
            }),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut url = std::env::var("DATABASE_URL").ok();
    let mut target: Option<i64> = None;
    let mut show_status = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--database-url" => url = Some(args.next().context("--database-url needs a value")?),
            "--target" => target = Some(args.next().context("--target needs a value")?.parse()?),
            "--status" => show_status = true,
            "-h" | "--help" => {
                println!("granary-migrate [--database-url URL] [--target N] [--status]");
                return Ok(());
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let url = granary_db::normalize_database_url(
        &url.context("set DATABASE_URL or pass --database-url")?,
    );
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;

    if show_status {
        for s in granary_db::migrate::status(&pool).await? {
            println!(
                "{:04} {:<40} {}{}",
                s.version,
                s.name,
                if s.applied { "applied" } else { "pending" },
                if s.checksum_mismatch {
                    " (file changed since applied)"
                } else {
                    ""
                }
            );
        }
        return Ok(());
    }

    let report = granary_db::migrate_to(&pool, target).await?;
    println!("applied {} migration(s)", report.applied.len());
    Ok(())
}
