//! routers/roles.py through the real app (template for router tests). No Python test covers these routes;
//! the HTTP suite (compat-tests/http-suite/tests/test_roles.py) pins the same behaviour on both stacks.

mod support;

use serde_json::{json, Value};
use support::{app, u, UserSpec};

const TAG: &str = "roles";

#[tokio::test]
async fn list_roles_returns_every_role_with_its_permission_codes() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let r = app.get(&sa.token, "/api/v1/roles").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json")
    );
    let roles = r.json();
    let roles = roles.as_array().unwrap();
    let mut db = app.db().await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM roles")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(roles.len() as i64 <= count && !roles.is_empty());
    let first = &roles[0];
    let keys: Vec<&str> = first
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "name",
            "slug",
            "description",
            "is_system_role",
            "permissions"
        ]
    );
    // The seeded executive role carries its permission codes.
    let exec = roles.iter().find(|r| r["slug"] == "executive").unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rpm JOIN roles r ON r.id = rpm.role_id WHERE r.slug = 'executive'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(exec["permissions"].as_array().unwrap().len() as i64, n);
}

#[tokio::test]
async fn anonymous_callers_get_401() {
    let app = app(TAG);
    let r = app.get("", "/api/v1/roles").await;
    assert_eq!(r.status, 401);
    assert_eq!(r.text(), r#"{"detail":"Not authenticated"}"#);
    let r = app.get("not-a-token", "/api/v1/roles/permissions").await;
    assert_eq!(r.text(), r#"{"detail":"Invalid token"}"#);
}

#[tokio::test]
async fn create_role_needs_a_system_admin_and_validates_first() {
    let app = app(TAG);
    let plain = app.user_with(&["crm:view_own"]).await;
    // Body errors come before the handler's own 403 (FastAPI validates the body first).
    let r = app.post(&plain.token, "/api/v1/roles", json!({})).await;
    assert_eq!(r.status, 422);
    assert_eq!(
        r.text(),
        r#"{"detail":"Validation failed","errors":[{"field":"name","message":"Field required"}],"code":"VALIDATION_ERROR"}"#
    );
    let r = app
        .post(&plain.token, "/api/v1/roles", json!({"name": u("r")}))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.text(), r#"{"detail":"System admin required"}"#);
}

#[tokio::test]
async fn system_admin_creates_a_role_with_permissions() {
    let app = app(TAG);
    let admin = app.make_user(UserSpec::new().system_admin()).await;
    let name = u("custom_sales");
    let r = app
        .post(
            &admin.token,
            "/api/v1/roles",
            json!({"name": name, "description": "d", "permission_codes": ["crm:view_own", "crm:view_own", "nope:x"]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let body = r.json();
    assert_eq!(body["name"], Value::String(name.clone()));
    assert_eq!(body.as_object().unwrap().len(), 2);
    let mut db = app.db().await;
    let (slug, codes): (String, Vec<String>) = sqlx::query_as(
        "SELECT r.slug, array(SELECT p.code FROM role_permissions rpm JOIN permissions p ON p.id = rpm.permission_id \
         WHERE rpm.role_id = r.id)::text[] FROM roles r WHERE r.id = $1::uuid",
    )
    .bind(body["id"].as_str().unwrap())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(slug, name);
    assert_eq!(codes, vec!["crm:view_own".to_string()]);

    // The same name again: ConflictError.
    let r = app
        .post(&admin.token, "/api/v1/roles", json!({"name": name}))
        .await;
    assert_eq!(r.status, 409);
    assert_eq!(
        r.json(),
        json!({"detail": format!("Role '{name}' already exists"), "code": "CONFLICT"})
    );
}

#[tokio::test]
async fn a_name_that_collides_with_a_slug_is_the_integrity_409() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    // A role whose slug differs from its name: posting that slug as a name passes the name check, then the
    // slug insert violates UNIQUE (seeded roles have name == slug).
    let slug = u("slug_only");
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, 'Display Name', $2, false)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(&slug)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post(&sa.token, "/api/v1/roles", json!({"name": slug}))
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(
        r.text(),
        r#"{"detail":"A record with these values already exists","code":"CONFLICT"}"#
    );
}

#[tokio::test]
async fn list_permissions_returns_code_name_description() {
    let app = app(TAG);
    let user = app.user_with(&[]).await;
    let r = app.get(&user.token, "/api/v1/roles/permissions").await;
    assert_eq!(r.status, 200);
    let perms = r.json();
    let p = perms
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["code"] == "crm:view_all")
        .unwrap();
    assert_eq!(p.as_object().unwrap().len(), 3);
    assert!(p["name"].is_string());
}

#[tokio::test]
async fn staff_chat_as_user_goes_through_the_real_router() {
    // services/nary/staff_chat.py `_as_user` -> granary_services::inproc (the app installed by build_app).
    let app = app(TAG);
    let plain = app.user_with(&["crm:view_own"]).await;
    let user = {
        let mut db = app.db().await;
        granary_core::auth::load_active_user(&mut db, plain.id)
            .await
            .unwrap()
            .unwrap()
    };
    let (status, body) = app
        .on_server(async move {
            granary_services::later::nary_staff_chat::as_user_request(
                &user,
                "POST",
                "/roles",
                Some(json!({"name": "x"})),
            )
            .await
            .unwrap()
        })
        .await;
    assert_eq!(status, 403);
    assert_eq!(body, br#"{"detail":"System admin required"}"#);
}

/// Stage 8: the role map and the permission catalogue are for staff; a portal account gets 403.
#[tokio::test]
async fn portal_users_cant_list_roles_or_permissions() {
    let app = app(TAG);
    for portal in ["partner", "vendor"] {
        let user = app.make_user(UserSpec::new().portal(portal)).await;
        for path in ["/api/v1/roles", "/api/v1/roles/permissions"] {
            let r = app.get(&user.token, path).await;
            assert_eq!(r.status, 403, "{portal} {path}: {}", r.text());
        }
    }
    let staff = app.user_with(&[]).await;
    assert_eq!(app.get(&staff.token, "/api/v1/roles").await.status, 200);
}

/// Granary: the Roles screen edits a custom role's description and permissions.
#[tokio::test]
async fn update_role_replaces_permissions_and_keeps_the_grant_rules() {
    let app = app(TAG);
    let admin = app
        .make_user(
            UserSpec::new()
                .system_admin()
                .perms(&["crm:view_own", "crm:view_all"]),
        )
        .await;
    let name = u("custom_edit");
    let r = app
        .post(
            &admin.token,
            "/api/v1/roles",
            json!({"name": name, "permission_codes": ["crm:view_own"]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/roles/{id}");

    // A plain user with the code still needs to be a system admin.
    let plain = app.user_with(&["crm:view_own"]).await;
    let r = app
        .patch(&plain.token, &path, json!({"description": "x"}))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.text(), r#"{"detail":"System admin required"}"#);

    // Replace the set with codes the admin holds: crm:view_own out, crm:view_all in.
    let r = app
        .patch(
            &admin.token,
            &path,
            json!({"description": "Sales lead", "permission_codes": ["crm:view_all", "nope:x"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"id": id, "name": name}));
    let mut db = app.db().await;
    let (description, codes): (Option<String>, Vec<String>) = sqlx::query_as(
        "SELECT r.description, array(SELECT p.code FROM role_permissions rpm JOIN permissions p \
         ON p.id = rpm.permission_id WHERE rpm.role_id = r.id ORDER BY p.code)::text[] \
         FROM roles r WHERE r.id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(description.as_deref(), Some("Sales lead"));
    assert_eq!(codes, vec!["crm:view_all".to_string()]);

    // Adding a code the admin doesn't hold is granting it: refused, nothing changes.
    let r = app
        .patch(
            &admin.token,
            &path,
            json!({"permission_codes": ["crm:view_all", "fleet:manage"]}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert!(r.text().contains("fleet:manage"), "{}", r.text());
    // The superadmin may.
    let sa = app.superadmin().await;
    let r = app
        .patch(
            &sa.token,
            &path,
            json!({"permission_codes": ["crm:view_all", "fleet:manage"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    // A code already on the role stays editable for the admin (keeping it is not granting it).
    let r = app
        .patch(
            &admin.token,
            &path,
            json!({"permission_codes": ["fleet:manage"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

#[tokio::test]
async fn update_role_refuses_built_in_roles_and_unknown_ids() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let exec: uuid::Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE slug = 'executive'")
        .fetch_one(&mut db)
        .await
        .unwrap();
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/roles/{exec}"),
            json!({"permission_codes": []}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/roles/{}", uuid::Uuid::new_v4()),
            json!({"description": "x"}),
        )
        .await;
    assert_eq!(r.status, 404, "{}", r.text());
}
