//! services/redis_lease.rs on the test Redis (127.0.0.1:56380, db 7).

mod support;

use redis::AsyncCommands;

use granary_services::redis_lease;

fn init() {
    std::env::set_var("REDIS_URL", "redis://127.0.0.1:56380/7");
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        ); // settings need one; unused
    }
}

/// test_audit_medium_w3.py::test_redis_lease_has_one_holder
#[tokio::test]
async fn test_redis_lease_has_one_holder() {
    init();
    let mut r = redis_lease::client().await.unwrap();
    let key = format!("lock:pytest.{}", uuid::Uuid::new_v4().simple());
    let a = redis_lease::new_token();
    let b = redis_lease::new_token();
    assert_eq!(a.len(), 32);
    assert!(a
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert!(redis_lease::acquire(&mut r, &key, &a, 30).await.unwrap());
    assert!(!redis_lease::acquire(&mut r, &key, &b, 30).await.unwrap());
    assert!(!redis_lease::renew(&mut r, &key, &b, 30).await.unwrap());
    redis_lease::release(&mut r, &key, &b).await.unwrap(); // not the holder: no effect
    let held: Option<String> = r.get(&key).await.unwrap();
    assert_eq!(held.as_deref(), Some(a.as_str()));
    assert!(redis_lease::renew(&mut r, &key, &a, 30).await.unwrap());
    redis_lease::release(&mut r, &key, &a).await.unwrap();
    assert!(redis_lease::acquire(&mut r, &key, &b, 30).await.unwrap());
    let _: () = r.del(&key).await.unwrap();
}

/// TTL semantics: SET EX on acquire, EXPIRE on renew; an expired lease can be taken by anyone and the old
/// holder can neither renew nor release it.
#[tokio::test]
async fn ttl_semantics() {
    init();
    let mut r = redis_lease::client().await.unwrap();
    let key = format!("lock:pytest.{}", uuid::Uuid::new_v4().simple());
    assert!(redis_lease::acquire(&mut r, &key, "a", 30).await.unwrap());
    let ttl: i64 = r.ttl(&key).await.unwrap();
    assert!((29..=30).contains(&ttl), "{ttl}");
    assert!(redis_lease::renew(&mut r, &key, "a", 120).await.unwrap());
    let ttl: i64 = r.ttl(&key).await.unwrap();
    assert!((119..=120).contains(&ttl), "{ttl}");
    let _: () = r.del(&key).await.unwrap();

    assert!(redis_lease::acquire(&mut r, &key, "old", 1).await.unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    assert!(!redis_lease::renew(&mut r, &key, "old", 30).await.unwrap());
    assert!(redis_lease::acquire(&mut r, &key, "new", 30).await.unwrap());
    redis_lease::release(&mut r, &key, "old").await.unwrap();
    let held: Option<String> = r.get(&key).await.unwrap();
    assert_eq!(held.as_deref(), Some("new"));
    // The shared connection works too.
    let mut shared = granary_core::redis_client::get_redis()
        .conn()
        .await
        .unwrap();
    assert!(!redis_lease::acquire(&mut shared, &key, "x", 30)
        .await
        .unwrap());
    redis_lease::release(&mut shared, &key, "new")
        .await
        .unwrap();
    let held: Option<String> = r.get(&key).await.unwrap();
    assert_eq!(held, None);
}
