//! granary-seed: the database seed (port of `python -m app.seed`). Idempotent; run it on every start.
//!
//!   granary-seed [--database-url URL]
//!
//! The URL defaults to $DATABASE_URL (a Python-style postgresql+asyncpg:// URL is accepted).
//! SUPERADMIN_EMAIL / SUPERADMIN_NAME / SUPERADMIN_PASSWORD come from the environment or `.env`.

use anyhow::{bail, Context, Result};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<()> {
    let mut url = std::env::var("DATABASE_URL").ok();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--database-url" => url = Some(args.next().context("--database-url needs a value")?),
            "-h" | "--help" => {
                println!("granary-seed [--database-url URL]");
                return Ok(());
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let env = granary_db::seed::SeedEnv::from_env()?;
    let url = granary_db::normalize_database_url(
        &url.context("set DATABASE_URL or pass --database-url")?,
    );
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    granary_db::seed::run(&pool, &env).await?;
    Ok(())
}
