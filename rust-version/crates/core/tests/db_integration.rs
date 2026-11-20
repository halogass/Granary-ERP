//! settings_store, clock and the auth dependencies against a database of its own (created fresh on the test
//! Postgres, migrated and seeded, plus one internal user with a role) and the test Redis (only `perms:{id}`
//! keys of the users touched, deleted afterwards).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::routing::get;
use axum::{Json, Router};
use granary_core::auth::{CurrentUser, InternalUser, OptionalUser};
use granary_core::db::Db;
use granary_core::state::{normalize_database_url, AppState, Redis};
use granary_core::{clock, settings_store, ApiError};
use redis::AsyncCommands;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_SECRET: &str = "test-jwt-secret-0123456789abcdef0123";

/// The database this binary works in: dropped, created, migrated and seeded once per run.
const DB_NAME: &str = "granary_core_db_integration";

async fn prepare_db() -> String {
    static URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    URL.get_or_init(|| async {
        use sqlx::Connection;
        let base = std::env::var("GRANARY_TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://granary:granarytest@127.0.0.1:55433/granary_test".into()
        });
        let base = normalize_database_url(&base);
        let root = base.rsplit_once('/').unwrap().0;
        let mut admin = sqlx::PgConnection::connect(&format!("{root}/postgres"))
            .await
            .expect("test Postgres reachable");
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{DB_NAME}\" WITH (FORCE)"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE DATABASE \"{DB_NAME}\""))
            .execute(&mut admin)
            .await
            .unwrap();
        let url = format!("{root}/{DB_NAME}");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        granary_db::migrate(&pool).await.expect("migrate");
        let env = granary_db::seed::SeedEnv {
            superadmin_email: "owner@example.com".into(),
            superadmin_name: "Alex Owner".into(),
            superadmin_password: "test-superadmin-password-1234".into(),
        };
        granary_db::seed::run(&pool, &env).await.expect("seed");
        // An internal user with a role that grants permissions, for the auth dependencies.
        let uid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, is_superadmin, is_system_admin, is_active, \
             portal_type, must_change_password, email_signature_enabled) \
             VALUES ($1, 'staff@example.com', 'Rachel Green', 'x', false, false, true, 'internal', false, true)",
        )
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_roles (user_id, role_id) SELECT $1, role_id FROM role_permissions \
             GROUP BY role_id ORDER BY count(*), role_id LIMIT 1",
        )
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        url
    })
    .await
    .clone()
}

fn settings(db: &str) -> granary_core::config::Settings {
    let redis = std::env::var("GRANARY_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:56380/0".into());
    let mut m = HashMap::new();
    for (k, v) in [
        ("app_secret_key", "test-app-secret"),
        ("database_url", db),
        ("postgres_password", "granarytest"),
        ("jwt_secret_key", JWT_SECRET),
        ("superadmin_password", "x"),
        ("minio_secret_key", "x"),
        ("redis_url", redis.as_str()),
    ] {
        m.insert(k.to_string(), v.to_string());
    }
    granary_core::config::Settings::from_map(&m).unwrap()
}

async fn state() -> AppState {
    let s = Arc::new(settings(&prepare_db().await));
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&normalize_database_url(&s.DATABASE_URL))
        .await
        .expect("test database reachable");
    AppState {
        db,
        redis: Arc::new(Redis::open(&s.REDIS_URL).unwrap()),
        settings: s,
    }
}

#[tokio::test]
async fn settings_store_and_clock() {
    let st = state().await;
    let mut tx = st.db.begin().await.unwrap();
    // The test company's time zone (inside this transaction only; the seed default is UTC).
    sqlx::query("UPDATE system_settings SET value = 'Asia/Tokyo' WHERE key = 'timezone'")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        settings_store::currency_places(&mut tx, Some("USD"))
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        settings_store::currency_places(&mut tx, Some("jpy"))
            .await
            .unwrap(),
        0
    );
    let e: ApiError = settings_store::currency_places(&mut tx, Some("XXX"))
        .await
        .unwrap_err()
        .into();
    assert_eq!(
        serde_json::to_string(&e.body()).unwrap(),
        r#"{"detail":"Setting 'currency.decimals' has no entry for XXX. Set it in Settings → Money & tax → Accounting."}"#
    );
    assert_eq!(
        settings_store::get_optional(&mut tx, "timezone")
            .await
            .unwrap()
            .as_deref(),
        Some("Asia/Tokyo")
    );
    assert_eq!(
        settings_store::get_optional(&mut tx, "no.such.setting")
            .await
            .unwrap(),
        None
    );
    let e: ApiError =
        settings_store::require_decimal(&mut tx, "no.such.setting", "Money & tax → Accounting")
            .await
            .unwrap_err()
            .into();
    assert_eq!(
        e.body()["detail"],
        "Setting 'no.such.setting' is not set. Set it in Settings → Money & tax → Accounting."
    );
    assert_eq!(
        clock::local_tz(&mut tx).await.unwrap(),
        chrono_tz::Asia::Tokyo
    );
    // An encrypted setting that is blank reads as unset.
    assert_eq!(
        settings_store::get_secret(&mut tx, "nary.google.client_secret", "test-app-secret")
            .await
            .unwrap(),
        None
    );
    tx.rollback().await.unwrap();
}

async fn pick_user(st: &AppState, sql: &str) -> Option<Uuid> {
    sqlx::query_as::<_, (Uuid,)>(sql)
        .fetch_optional(&st.db)
        .await
        .unwrap()
        .map(|r| r.0)
}

fn token(sub: &str) -> String {
    let cfg = granary_compat::crypto::jwt::JwtConfig::new(JWT_SECRET);
    granary_compat::crypto::jwt::create_access_token(&cfg, sub, None).unwrap()
}

#[tokio::test]
async fn auth_dependencies() {
    let st = state().await;
    let app =
        granary_core::http::finish_app(
            Router::new()
                .route(
                    "/me",
                    get(|CurrentUser(u): CurrentUser| async move {
                        let mut p: Vec<String> = u.perms.into_iter().collect();
                        p.sort();
                        Json(json!({"email": u.email, "perms": p}))
                    }),
                )
                .route(
                    "/internal",
                    get(|InternalUser(u): InternalUser| async move { Json(json!({"id": u.id})) }),
                )
                .route(
                    "/optional",
                    get(|OptionalUser(u): OptionalUser| async move {
                        Json(json!({"id": u.map(|u| u.id)}))
                    }),
                )
                .route(
                    "/db",
                    get(|db: Db| async move {
                        let n: (i64,) = sqlx::query_as("SELECT count(*) FROM roles")
                            .fetch_one(&mut *db.conn().await?)
                            .await?;
                        Ok::<_, ApiError>(Json(json!({"roles": n.0})))
                    }),
                ),
            st.clone(),
            granary_core::http::Cors::new(vec![]),
        );
    let call = |path: &str, auth: Option<String>| {
        let mut req = Request::builder().uri(path);
        if let Some(a) = auth {
            req = req.header("authorization", a);
        }
        let app = app.clone();
        let req = req.body(Body::empty()).unwrap();
        async move {
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status().as_u16();
            let bytes = http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes();
            (status, String::from_utf8_lossy(&bytes).to_string())
        }
    };

    assert_eq!(
        call("/me", None).await,
        (401, r#"{"detail":"Not authenticated"}"#.into())
    );
    assert_eq!(
        call("/me", Some("Basic abc".into())).await,
        (401, r#"{"detail":"Not authenticated"}"#.into())
    );
    assert_eq!(
        call("/me", Some("Bearer nope".into())).await,
        (401, r#"{"detail":"Invalid token"}"#.into())
    );
    let ghost = Uuid::new_v4().to_string();
    assert_eq!(
        call("/me", Some(format!("Bearer {}", token(&ghost)))).await,
        (401, r#"{"detail":"User not found"}"#.into())
    );
    assert_eq!(
        call("/optional", None).await,
        (200, r#"{"id":null}"#.into())
    );
    assert_eq!(call("/db", None).await.0, 200);

    let mut redis = st.redis.conn().await.expect("test redis reachable");

    // An internal user with roles: permissions equal the union of role and direct grants, and the cache
    // holds Python's json.dumps(sorted(perms)).
    let uid = pick_user(
        &st,
        "SELECT u.id FROM users u JOIN user_roles ur ON ur.user_id = u.id \
          WHERE u.is_active AND NOT u.is_superadmin AND u.portal_type = 'internal' LIMIT 1",
    )
    .await
    .expect("an internal user with a role");
    let key = format!("perms:{uid}");
    let _: () = redis.del(&key).await.unwrap();
    let (status, body) = call("/me", Some(format!("Bearer {}", token(&uid.to_string())))).await;
    assert_eq!(status, 200, "{body}");
    let got: serde_json::Value = serde_json::from_str(&body).unwrap();
    let want: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT code FROM (SELECT p.code FROM permissions p JOIN role_permissions rpm ON rpm.permission_id = p.id \
           JOIN user_roles ur ON ur.role_id = rpm.role_id WHERE ur.user_id = $1 \
         UNION ALL SELECT p.code FROM permissions p JOIN user_permissions up ON up.permission_id = p.id \
          WHERE up.user_id = $1) x ORDER BY code",
    )
    .bind(uid)
    .fetch_all(&st.db)
    .await
    .unwrap();
    let want: Vec<String> = want.into_iter().map(|r| r.0).collect();
    assert_eq!(got["perms"], json!(want));
    let cached: Option<String> = redis.get(&key).await.unwrap();
    let quoted: Vec<String> = want.iter().map(|c| format!("\"{c}\"")).collect();
    assert_eq!(cached.unwrap(), format!("[{}]", quoted.join(", ")));
    let ttl: i64 = redis.ttl(&key).await.unwrap();
    assert!(ttl > 0 && ttl <= 120);
    // Served from the cache the second time.
    let _: () = redis.set_ex(&key, r#"["only:cached"]"#, 120).await.unwrap();
    let (_, body) = call("/me", Some(format!("Bearer {}", token(&uid.to_string())))).await;
    assert!(body.contains(r#""perms":["only:cached"]"#), "{body}");
    let _: () = redis.del(&key).await.unwrap();
    assert_eq!(
        call(
            "/internal",
            Some(format!("Bearer {}", token(&uid.to_string())))
        )
        .await
        .0,
        200
    );

    // A portal user is refused by the internal-only gate.
    if let Some(pid) = pick_user(
        &st,
        "SELECT id FROM users WHERE is_active AND portal_type IN ('partner', 'vendor') LIMIT 1",
    )
    .await
    {
        assert_eq!(
            call(
                "/internal",
                Some(format!("Bearer {}", token(&pid.to_string())))
            )
            .await,
            (403, r#"{"detail":"Internal users only"}"#.into())
        );
        let _: () = redis.del(format!("perms:{pid}")).await.unwrap();
    }

    // A refresh token is not an access token.
    let cfg = granary_compat::crypto::jwt::JwtConfig::new(JWT_SECRET);
    let refresh =
        granary_compat::crypto::jwt::create_refresh_token(&cfg, &uid.to_string()).unwrap();
    assert_eq!(
        call("/me", Some(format!("Bearer {refresh}"))).await,
        (401, r#"{"detail":"Invalid token"}"#.into())
    );
}

/// A NUMERIC zero keeps its display scale, as asyncpg's `Decimal('0.00')` does (vendored sqlx-postgres
/// patch; upstream decodes every zero as a scale-0 `0`).
#[tokio::test]
async fn numeric_zero_keeps_scale() {
    let st = state().await;
    let row: (
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
    ) = sqlx::query_as(
        "SELECT 0::numeric(14,2), 0::numeric, 0.000::numeric(10,3), 12.50::numeric(14,2)",
    )
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(row.0.to_string(), "0.00");
    assert_eq!(row.1.to_string(), "0");
    assert_eq!(row.2.to_string(), "0.000");
    assert_eq!(row.3.to_string(), "12.50");
}

/// `get_redis()` on a refused port fails at once (redis-py raises immediately), no connect retries.
#[tokio::test]
async fn redis_refused_fails_fast() {
    let r = Redis::open("redis://127.0.0.1:9/0").unwrap();
    let t0 = std::time::Instant::now();
    assert!(r.conn().await.is_err());
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        t0.elapsed()
    );
}
