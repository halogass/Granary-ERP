//! 20 Nov 2026: the project page edits the customer and the project manager and manages the team
//! (PATCH /projects/{id} with customer_org_id / pm_id, POST/DELETE /projects/{id}/members).

mod support;

use serde_json::json;
use uuid::Uuid;

async fn new_project(app: &support::TestApp, token: &str) -> String {
    let res = app
        .post(
            token,
            "/api/v1/projects",
            json!({"name": support::u("Team")}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    res.json()["id"].as_str().unwrap().to_string()
}

async fn new_org(app: &support::TestApp) -> Uuid {
    let id = Uuid::new_v4();
    let mut db = app.db().await;
    sqlx::query("INSERT INTO organizations (id, name, is_deleted) VALUES ($1, $2, false)")
        .bind(id)
        .bind(support::u("Org"))
        .execute(&mut db)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn test_edit_customer_and_pm_shows_on_the_project() {
    let app = support::app("projects_team");
    let sa = app.superadmin().await;
    let editor = app
        .user_with(&["projects:create", "projects:edit", "projects:view_all"])
        .await;
    let pm = app.user_with(&[]).await;
    let pid = new_project(app, &editor.token).await;
    let org = new_org(app).await;

    let res = app
        .patch(
            &editor.token,
            &format!("/api/v1/projects/{pid}"),
            json!({"status": "active", "customer_org_id": org.to_string(), "pm_id": pm.id.to_string()}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());

    let got = app
        .get(&sa.token, &format!("/api/v1/projects/{pid}"))
        .await
        .json();
    assert_eq!(got["status"], "active");
    assert_eq!(got["customer_org_id"], org.to_string());
    assert_eq!(got["customer_org"]["id"], org.to_string());
    assert_eq!(got["pm_id"], pm.id.to_string());
    assert_eq!(got["pm_user"]["email"], pm.email);
    // the new PM joins the team; the creator was added on create
    let members: Vec<String> = got["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user_id"].as_str().unwrap().to_string())
        .collect();
    assert!(members.contains(&pm.id.to_string()), "{got}");
    assert!(members.contains(&editor.id.to_string()), "{got}");

    // the PM can't be taken off the team while they are the PM
    let res = app
        .delete(
            &editor.token,
            &format!("/api/v1/projects/{pid}/members/{}", pm.id),
        )
        .await;
    assert_eq!(res.status, 400, "{}", res.text());

    // clearing the PM and the customer
    let res = app
        .patch(
            &editor.token,
            &format!("/api/v1/projects/{pid}"),
            json!({"customer_org_id": null, "pm_id": null}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let got = app
        .get(&sa.token, &format!("/api/v1/projects/{pid}"))
        .await
        .json();
    assert!(
        got["pm_user"].is_null() && got["customer_org"].is_null(),
        "{got}"
    );
}

#[tokio::test]
async fn test_pm_and_customer_must_exist() {
    let app = support::app("projects_team");
    let sa = app.superadmin().await;
    let pid = new_project(app, &sa.token).await;
    let vendor = app
        .make_user(support::UserSpec::new().portal("vendor"))
        .await;
    for body in [
        json!({"pm_id": Uuid::new_v4().to_string()}),
        json!({"pm_id": vendor.id.to_string()}),
        json!({"customer_org_id": Uuid::new_v4().to_string()}),
    ] {
        let res = app
            .patch(&sa.token, &format!("/api/v1/projects/{pid}"), body.clone())
            .await;
        assert_eq!(res.status, 400, "{body}: {}", res.text());
    }
}

#[tokio::test]
async fn test_add_and_remove_members() {
    let app = support::app("projects_team");
    let sa = app.superadmin().await;
    let editor = app.user_with(&["projects:create", "projects:edit"]).await;
    let person = app.user_with(&[]).await;
    let pid = new_project(app, &editor.token).await;
    let path = format!("/api/v1/projects/{pid}/members");

    let res = app
        .post(
            &editor.token,
            &path,
            json!({"user_id": person.id.to_string(), "role": "engineer", "allocation_percentage": 50}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let res = app
        .post(
            &editor.token,
            &path,
            json!({"user_id": person.id.to_string()}),
        )
        .await;
    assert_eq!(res.status, 409, "{}", res.text());
    let res = app
        .post(
            &editor.token,
            &path,
            json!({"user_id": Uuid::new_v4().to_string(), "allocation_percentage": 150}),
        )
        .await;
    assert_eq!(res.status, 422, "{}", res.text());

    let got = app
        .get(&sa.token, &format!("/api/v1/projects/{pid}"))
        .await
        .json();
    let m = got["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["user_id"] == person.id.to_string())
        .cloned()
        .unwrap();
    assert_eq!(m["role"], "engineer");
    assert_eq!(m["allocation_percentage"], 50);
    // a member now sees the project
    let res = app
        .get(&person.token, &format!("/api/v1/projects/{pid}"))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());

    let res = app
        .delete(&editor.token, &format!("{path}/{}", person.id))
        .await;
    assert_eq!(res.status, 204, "{}", res.text());
    let res = app
        .delete(&editor.token, &format!("{path}/{}", person.id))
        .await;
    assert_eq!(res.status, 404, "{}", res.text());
}

#[tokio::test]
async fn test_team_changes_need_projects_edit() {
    let app = support::app("projects_team");
    let sa = app.superadmin().await;
    let pid = new_project(app, &sa.token).await;
    let viewer = app.user_with(&["projects:view_all"]).await;
    let res = app
        .post(
            &viewer.token,
            &format!("/api/v1/projects/{pid}/members"),
            json!({"user_id": viewer.id.to_string()}),
        )
        .await;
    assert_eq!(res.status, 403);
    let res = app
        .delete(
            &viewer.token,
            &format!("/api/v1/projects/{pid}/members/{}", sa.id),
        )
        .await;
    assert_eq!(res.status, 403);
    let res = app
        .patch(
            &viewer.token,
            &format!("/api/v1/projects/{pid}"),
            json!({"pm_id": viewer.id.to_string()}),
        )
        .await;
    assert_eq!(res.status, 403);
}

/// Batch E: a member's role and allocation are edited in place (PATCH /projects/{id}/members/{user_id}).
#[tokio::test]
async fn test_edit_member_role_and_allocation_in_place() {
    let app = support::app("projects_team");
    let sa = app.superadmin().await;
    let editor = app.user_with(&["projects:create", "projects:edit"]).await;
    let viewer = app.user_with(&["projects:view_all"]).await;
    let person = app.user_with(&[]).await;
    let pm = app.user_with(&[]).await;
    let pid = new_project(app, &editor.token).await;
    let path = format!("/api/v1/projects/{pid}/members");
    let res = app
        .post(
            &editor.token,
            &path,
            json!({"user_id": person.id.to_string(), "role": "member", "allocation_percentage": 50}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let member_path = format!("{path}/{}", person.id);

    let member = |got: &serde_json::Value, id: Uuid| {
        got["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user_id"] == id.to_string())
            .cloned()
            .unwrap()
    };

    // role only: the allocation stays
    let res = app
        .patch(&editor.token, &member_path, json!({"role": "tech_lead"}))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let got = app
        .get(&sa.token, &format!("/api/v1/projects/{pid}"))
        .await
        .json();
    let m = member(&got, person.id);
    assert_eq!(m["role"], "tech_lead");
    assert_eq!(m["allocation_percentage"], 50);

    // allocation only, then cleared with null
    let res = app
        .patch(
            &editor.token,
            &member_path,
            json!({"allocation_percentage": 80}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let got = app
        .get(&sa.token, &format!("/api/v1/projects/{pid}"))
        .await
        .json();
    assert_eq!(member(&got, person.id)["allocation_percentage"], 80);
    assert_eq!(member(&got, person.id)["role"], "tech_lead");
    let res = app
        .patch(
            &editor.token,
            &member_path,
            json!({"allocation_percentage": null}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let got = app
        .get(&sa.token, &format!("/api/v1/projects/{pid}"))
        .await
        .json();
    assert!(
        member(&got, person.id)["allocation_percentage"].is_null(),
        "{got}"
    );

    // bad values
    for body in [
        json!({"allocation_percentage": 150}),
        json!({"allocation_percentage": -1}),
        json!({"allocation_percentage": "50"}),
        json!({"role": ""}),
        json!({"role": "x".repeat(51)}),
    ] {
        let res = app.patch(&editor.token, &member_path, body.clone()).await;
        assert_eq!(res.status, 422, "{body}: {}", res.text());
    }

    // someone not on the team
    let res = app
        .patch(
            &editor.token,
            &format!("{path}/{}", Uuid::new_v4()),
            json!({"role": "member"}),
        )
        .await;
    assert_eq!(res.status, 404, "{}", res.text());

    // needs projects:edit
    let res = app
        .patch(&viewer.token, &member_path, json!({"role": "member"}))
        .await;
    assert_eq!(res.status, 403, "{}", res.text());

    // the project manager keeps the project_manager role, but their allocation can change
    let res = app
        .patch(
            &editor.token,
            &format!("/api/v1/projects/{pid}"),
            json!({"pm_id": pm.id.to_string()}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let pm_path = format!("{path}/{}", pm.id);
    let res = app
        .patch(&editor.token, &pm_path, json!({"role": "member"}))
        .await;
    assert_eq!(res.status, 400, "{}", res.text());
    let res = app
        .patch(
            &editor.token,
            &pm_path,
            json!({"allocation_percentage": 30}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let got = app
        .get(&sa.token, &format!("/api/v1/projects/{pid}"))
        .await
        .json();
    let m = member(&got, pm.id);
    assert_eq!(m["role"], "project_manager");
    assert_eq!(m["allocation_percentage"], 30);
}
