mod support;

#[tokio::test]
async fn fresh_db_is_migrated_and_seeded() {
    let pool = support::fresh_db("smoke").await;
    let n: i64 = sqlx::query_scalar("select count(*) from system_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(n > 200, "{n}");
}
