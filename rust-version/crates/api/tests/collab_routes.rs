//! routers/collab.py: the backend test that writes rows directly (test_audit_medium_w3.py M58).
//! The HTTP-only tests are in compat-tests/http-suite/tests/test_collab_routes.py.

mod support;

use uuid::Uuid;

/// test_audit_medium_w3.py::test_collab_needs_project_membership.
#[tokio::test]
async fn test_collab_needs_project_membership() {
    let app = support::app("collab");
    let member = app.make_user(support::UserSpec::new()).await;
    let outsider = app.make_user(support::UserSpec::new()).await;
    let mut db = app.db().await;
    let (project, task) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO projects (id, name, project_type, is_deleted) VALUES ($1, $2, 'other', false)",
    )
    .bind(project)
    .bind(support::u("Pytest W3 project"))
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_tasks (id, project_id, title, is_deleted) VALUES ($1, $2, 'Task', false)",
    )
    .bind(task)
    .bind(project)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_members (id, project_id, user_id, role) VALUES ($1, $2, $3, 'member')",
    )
    .bind(Uuid::new_v4())
    .bind(project)
    .bind(member.id)
    .execute(&mut db)
    .await
    .unwrap();
    let q = format!("entity_type=project_task&entity_id={task}");
    let c = |p: &str| format!("/api/v1/collab/{p}");
    assert_eq!(
        app.get(&outsider.token, &c(&format!("comments?{q}")))
            .await
            .status,
        403
    );
    assert_eq!(
        app.post(
            &outsider.token,
            &c("comments"),
            serde_json::json!({"entity_type": "project_task", "entity_id": task.to_string(), "body": "hi"}),
        )
        .await
        .status,
        403
    );
    assert_eq!(
        app.get(
            &outsider.token,
            &c(&format!("epics?scope_type=project&scope_id={project}"))
        )
        .await
        .status,
        403
    );
    assert_eq!(
        app.get(&member.token, &c(&format!("comments?{q}")))
            .await
            .status,
        200
    );
    let viewer = app.user_with(&["projects:view_all"]).await;
    assert_eq!(
        app.get(&viewer.token, &c(&format!("activity?{q}")))
            .await
            .status,
        200
    );
}

/// The project's PM may read its board without being a member row.
#[tokio::test]
async fn test_the_pm_reads_the_board() {
    let app = support::app("collab");
    let pm = app.make_user(support::UserSpec::new()).await;
    let mut db = app.db().await;
    let project = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO projects (id, name, project_type, pm_id, is_deleted) VALUES ($1, 'PM project', 'other', $2, false)",
    )
    .bind(project)
    .bind(pm.id)
    .execute(&mut db)
    .await
    .unwrap();
    let res = app
        .get(
            &pm.token,
            &format!("/api/v1/collab/columns?scope_type=project&scope_id={project}"),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json(), serde_json::json!([]));
}
