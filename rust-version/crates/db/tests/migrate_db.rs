//! Database tests for the runner, the row structs and the pgvector codec.
//! Skipped unless GRANARY_DB_TEST_URL points at a scratch database the test may drop and recreate,
//! e.g. GRANARY_DB_TEST_URL=postgres://granary:granarytest@127.0.0.1:55433/granary_schema_rs_test

use granary_db::rows;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection};

async fn fresh_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("GRANARY_DB_TEST_URL").ok()?;
    let (base, db) = url.rsplit_once('/').expect("url with database name");
    assert!(
        !matches!(db, "granary_test" | "granary_upg" | "postgres"),
        "refusing to drop {db}"
    );
    let mut admin = PgConnection::connect(&format!("{base}/postgres"))
        .await
        .unwrap();
    sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS \"{db}\" WITH (FORCE)"))
        .execute(&mut admin)
        .await
        .unwrap();
    sqlx::raw_sql(&format!("CREATE DATABASE \"{db}\""))
        .execute(&mut admin)
        .await
        .unwrap();
    Some(
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn migrate_rows_and_vector() {
    let Some(pool) = fresh_pool().await else {
        eprintln!("GRANARY_DB_TEST_URL not set; skipping");
        return;
    };
    let report = granary_db::migrate(&pool).await.unwrap();
    assert_eq!(report.applied.first(), Some(&1));
    // Idempotent.
    let again = granary_db::migrate(&pool).await.unwrap();
    assert!(again.applied.is_empty());

    // Row struct decode.
    sqlx::query("INSERT INTO accounts (id, code, name, category, is_deleted) VALUES (gen_random_uuid(), '1000', 'Cash', 'asset', false)")
        .execute(&pool).await.unwrap();
    let sql = format!(
        "SELECT {} FROM {}",
        rows::finance::Account::COLUMNS.join(", "),
        rows::finance::Account::TABLE
    );
    let acct: rows::finance::Account = sqlx::query_as(&sql).fetch_one(&pool).await.unwrap();
    assert_eq!(acct.code, "1000");

    // pgvector round trip (binary both ways).
    let v = granary_db::Vector(vec![1.5, -2.0, 0.25]);
    let back: granary_db::Vector = sqlx::query_scalar("SELECT $1::vector")
        .bind(&v)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(back, v);
    let text: String = sqlx::query_scalar("SELECT $1::vector::text")
        .bind(&v)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(text, "[1.5,-2,0.25]");
}
