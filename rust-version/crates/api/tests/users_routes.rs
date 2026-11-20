//! routers/users.py through the real app. The HTTP-only tests live in compat-tests/http-suite
//! (test_users_admin.py, test_users_routes.py); these read Redis and rows directly.

mod support;

use redis::AsyncCommands;
use serde_json::json;
use support::{app, u, UserSpec};
use uuid::Uuid;

const TAG: &str = "users";

async fn cached(app: &support::TestApp, user: Uuid) -> Option<String> {
    let c = redis::Client::open(app.redis_url.as_str()).unwrap();
    let mut r = c.get_multiplexed_async_connection().await.unwrap();
    r.get(format!("perms:{user}")).await.unwrap()
}

/// Role changes and password resets drop the user's cached permission set.
#[tokio::test]
async fn role_changes_and_resets_invalidate_the_permission_cache() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let user = app.user_with(&["crm:view_own"]).await;
    // Fill the cache with a request as the user.
    assert_eq!(app.get(&user.token, "/api/v1/auth/me").await.status, 200);
    assert_eq!(
        cached(app, user.id).await.as_deref(),
        Some("[\"crm:view_own\"]")
    );

    let role = u("pytest-cache");
    let mut db = app.db().await;
    let role_id = Uuid::new_v4();
    sqlx::query("INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $3, false)")
        .bind(role_id)
        .bind(&role)
        .bind(role.replace('-', "_"))
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/users/{}/roles/{role}", user.id),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(cached(app, user.id).await, None);

    assert_eq!(app.get(&user.token, "/api/v1/auth/me").await.status, 200);
    assert!(cached(app, user.id).await.is_some());
    let r = app
        .delete(
            &sa.token,
            &format!("/api/v1/users/{}/roles/{role}", user.id),
        )
        .await;
    assert_eq!(r.status, 204);
    assert_eq!(cached(app, user.id).await, None);

    assert_eq!(app.get(&user.token, "/api/v1/auth/me").await.status, 200);
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/users/{}/reset-password", user.id),
            json!({"new_password": "pytest-new-123"}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(cached(app, user.id).await, None);
    let (must, revoked): (Option<bool>, i64) = sqlx::query_as(
        "SELECT must_change_password, (SELECT count(*) FROM user_sessions WHERE user_id = $1 AND is_revoked = false) \
         FROM users WHERE id = $1",
    )
    .bind(user.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((must, revoked), (Some(true), 0));
}

/// Deactivating a user terminates the linked employee with today's company date.
#[tokio::test]
async fn deactivation_ends_the_employee_today() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let user = app.make_user(UserSpec::new()).await;
    let mut db = app.db().await;
    let emp = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO employees (id, user_id, first_name, email, status, is_deleted) \
         VALUES ($1, $2, 'Gone', $3, 'active', false)",
    )
    .bind(emp)
    .bind(user.id)
    .bind(&user.email)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .delete(&sa.token, &format!("/api/v1/users/{}", user.id))
        .await;
    assert_eq!(r.status, 204, "{}", r.text());
    let today = granary_core::clock::today_local(&mut db).await.unwrap();
    let (status, end): (String, Option<chrono::NaiveDate>) =
        sqlx::query_as("SELECT status, end_date FROM employees WHERE id = $1")
            .bind(emp)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((status.as_str(), end), ("terminated", Some(today)));
    // A deactivated user's token stops working.
    assert_eq!(app.get(&user.token, "/api/v1/auth/me").await.status, 401);
}

/// Two non-deleted employees linked to one user: scalar_one_or_none raises (a 500), as in Python.
#[tokio::test]
async fn two_linked_employees_are_a_500() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let user = app.make_user(UserSpec::new()).await;
    let mut db = app.db().await;
    for _ in 0..2 {
        sqlx::query(
            "INSERT INTO employees (id, user_id, first_name, email, status, is_deleted) \
             VALUES ($1, $2, 'Twin', $3, 'active', false)",
        )
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(&user.email)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/users/{}", user.id),
            json!({"name": "New"}),
        )
        .await;
    assert_eq!(r.status, 500);
    assert_eq!(
        r.json(),
        json!({"detail": "Internal server error", "code": "INTERNAL_ERROR"})
    );
    let name: String = sqlx::query_scalar("SELECT name FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_ne!(name, "New", "the request was rolled back");
}

/// A role with exactly `perms` (name == slug).
async fn role_with(app: &support::TestApp, perms: &[&str]) -> String {
    let name = u("pytest_grant").replace('-', "_");
    let mut db = app.db().await;
    let role_id = Uuid::new_v4();
    sqlx::query("INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $2, false)")
        .bind(role_id)
        .bind(&name)
        .execute(&mut db)
        .await
        .unwrap();
    for code in perms {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) SELECT $1, id FROM permissions WHERE code = $2",
        )
        .bind(role_id)
        .bind(code)
        .execute(&mut db)
        .await
        .unwrap();
    }
    name
}

/// Stage 8: a system admin (not the superadmin) hands out only roles whose permissions they hold, never the
/// superadmin role; the superadmin may grant anything. Creating a user with roles follows the same rule.
#[tokio::test]
async fn admins_only_grant_what_they_hold() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let admin = app
        .make_user(UserSpec::new().system_admin().perms(&["crm:view_own"]))
        .await;
    let target = app.make_user(UserSpec::new()).await;
    let held = role_with(app, &["crm:view_own"]).await;
    let more = role_with(app, &["crm:view_own", "crm:view_all"]).await;
    let tid = target.id;
    let assign = |token: String, role: String| async move {
        app.post(
            &token,
            &format!("/api/v1/users/{tid}/roles/{role}"),
            json!({}),
        )
        .await
    };
    let r = assign(admin.token.clone(), more.clone()).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert!(r.text().contains("crm:view_all"), "{}", r.text());
    let r = assign(admin.token.clone(), "superadmin".into()).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert!(r.text().contains("Only the superadmin"), "{}", r.text());
    assert_eq!(assign(admin.token.clone(), held.clone()).await.status, 200);
    assert_eq!(assign(sa.token.clone(), more.clone()).await.status, 200);
    let r = app
        .post(
            &admin.token,
            "/api/v1/users",
            json!({"email": format!("{}@example.com", u("grant")), "name": "New Person",
                   "password": "a-long-password-1", "role_names": ["superadmin"]}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    let mut db = app.db().await;
    let roles: Vec<(String,)> = sqlx::query_as(
        "SELECT roles.name FROM user_roles JOIN roles ON roles.id = user_roles.role_id \
         WHERE user_roles.user_id = $1 ORDER BY roles.name",
    )
    .bind(target.id)
    .fetch_all(&mut db)
    .await
    .unwrap();
    let mut want = vec![held, more];
    want.sort();
    assert_eq!(roles.into_iter().map(|r| r.0).collect::<Vec<_>>(), want);
}

/// Stage 8: an account on a temporary password reaches only who-am-I, the two password changes and logout.
#[tokio::test]
async fn a_temporary_password_blocks_everything_but_the_password_change() {
    let app = app(TAG);
    let user = app.user_with(&["crm:view_own"]).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET must_change_password = true WHERE id = $1")
        .bind(user.id)
        .execute(&mut db)
        .await
        .unwrap();
    let me = app.get(&user.token, "/api/v1/auth/me").await;
    assert_eq!(me.status, 200, "{}", me.text());
    assert_eq!(me.json()["must_change_password"], true);
    for path in ["/api/v1/users", "/api/v1/roles", "/api/v1/auth/totp"] {
        let r = app.get(&user.token, path).await;
        assert_eq!(r.status, 403, "{path}: {}", r.text());
        assert_eq!(r.json()["code"], "password_change_required", "{path}");
    }
    let r = app
        .patch(
            &user.token,
            "/api/v1/users/me/password",
            json!({"current_password": support::DEFAULT_PASSWORD, "new_password": "pytest-changed-1"}),
        )
        .await;
    assert!(r.status < 300, "{}", r.text());
    let must: Option<bool> =
        sqlx::query_scalar("SELECT must_change_password FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(must, Some(false));
    assert_eq!(app.get(&user.token, "/api/v1/roles").await.status, 200);
}
