//! routers/rnd.py: the GitLab calls (a wiremock GitLab) and the row-level effects the HTTP suite cannot
//! see. No backend test calls GitLab; these pin the Python route's behaviour as read from rnd.py.

mod support;

use serde_json::{json, Value};
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use support::{u, TestApp};

const R: &str = "/api/v1/rnd";

fn app() -> &'static TestApp {
    support::app("rnd")
}

/// A config pointing at `url` and a repo for GitLab project `gid`: (config id, repo id).
async fn config_and_repo(app: &TestApp, token: &str, url: &str, gid: i64) -> (String, String) {
    let cfg = app
        .post(
            token,
            &format!("{R}/gitlab/configs"),
            json!({"name": u("GL"), "url": format!("{url}/"),
              "access_token": "glpat-secret", "webhook_secret": "hook"}),
        )
        .await;
    assert_eq!(cfg.status, 201, "{}", cfg.text());
    let cfg_id = cfg.json()["id"].as_str().unwrap().to_string();
    let repo = app
        .post(
            token,
            &format!("{R}/gitlab/repos"),
            json!({"gitlab_config_id": cfg_id, "gitlab_project_id": gid,
              "full_path": "acme/face-api", "name": "face-api"}),
        )
        .await;
    assert_eq!(repo.status, 201, "{}", repo.text());
    (cfg_id, repo.json()["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn test_connection_caches_the_instance_version() {
    let app = app();
    let sa = app.superadmin().await;
    granary_services::safe_fetch::allow_loopback_for_tests();
    let gl = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/version"))
        .and(header("PRIVATE-TOKEN", "glpat-secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"version": "16.8.1", "revision": "abc123"})),
        )
        .mount(&gl)
        .await;
    let (cfg, _) = config_and_repo(app, &sa.token, &gl.uri(), 1).await;
    let res = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/configs/{cfg}/test"),
            json!({}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(
        res.json(),
        json!({"connected": true, "version": "16.8.1", "revision": "abc123"})
    );
    let list = app
        .get(&sa.token, &format!("{R}/gitlab/configs"))
        .await
        .json();
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == json!(cfg))
        .unwrap()
        .clone();
    assert_eq!(
        (
            row["instance_name"].clone(),
            row["instance_version"].clone()
        ),
        (json!("16.8.1"), json!("abc123"))
    );
}

#[tokio::test]
async fn test_connection_reports_http_errors_and_bad_json() {
    let app = app();
    let sa = app.superadmin().await;
    granary_services::safe_fetch::allow_loopback_for_tests();
    let gl = MockServer::start().await;
    let (cfg, _) = config_and_repo(app, &sa.token, &gl.uri(), 2).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/version"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .mount(&gl)
        .await;
    let res = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/configs/{cfg}/test"),
            json!({}),
        )
        .await;
    assert_eq!(res.json(), json!({"connected": false, "error": "HTTP 401"}));
    Mock::given(method("GET"))
        .and(path("/api/v4/version"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&gl)
        .await;
    let res = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/configs/{cfg}/test"),
            json!({}),
        )
        .await;
    assert_eq!(
        res.json(),
        json!({"connected": false, "error": "Expecting value: line 1 column 1 (char 0)"})
    );
    // No token: 400 before any call.
    let mut db = app.db().await;
    sqlx::query("UPDATE gitlab_configs SET access_token_encrypted = NULL WHERE id = $1::uuid")
        .bind(&cfg)
        .execute(&mut db)
        .await
        .unwrap();
    let res = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/configs/{cfg}/test"),
            json!({}),
        )
        .await;
    assert_eq!(
        (res.status, res.json()),
        (400, json!({"detail": "No access token configured"}))
    );
}

#[tokio::test]
async fn sync_pipelines_inserts_new_runs_and_updates_known_ones() {
    let app = app();
    let sa = app.superadmin().await;
    granary_services::safe_fetch::allow_loopback_for_tests();
    let gl = MockServer::start().await;
    let (_, repo) = config_and_repo(app, &sa.token, &gl.uri(), 77).await;
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO gitlab_pipeline_runs (id, repository_id, gitlab_pipeline_id, status) VALUES ($1, $2::uuid, 11, 'running')",
    )
    .bind(Uuid::new_v4())
    .bind(&repo)
    .execute(&mut db)
    .await
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/77/pipelines"))
        .and(query_param("per_page", "20"))
        .and(query_param("order_by", "id"))
        .and(query_param("sort", "desc"))
        .and(header("PRIVATE-TOKEN", "glpat-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 12, "status": "success", "ref": "main", "sha": "aaa", "source": "push",
             "web_url": "https://gl/p/12", "created_at": "2026-10-01T10:00:00.123Z"},
            {"id": 11, "status": "failed", "ref": "dev"},
        ])))
        .mount(&gl)
        .await;
    let res = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/repos/{repo}/sync-pipelines"),
            json!({}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json(), json!({"synced": 1, "total": 2}));
    let runs = app
        .get(&sa.token, &format!("{R}/gitlab/pipelines?repo_id={repo}"))
        .await
        .json();
    let by_id = |gid: i64| -> Value {
        runs.as_array()
            .unwrap()
            .iter()
            .find(|r| r["gitlab_pipeline_id"] == json!(gid))
            .unwrap()
            .clone()
    };
    assert_eq!(by_id(11)["status"], json!("failed"));
    let new = by_id(12);
    assert_eq!(
        (
            new["status"].clone(),
            new["ref"].clone(),
            new["started_at"].clone(),
            new["web_url"].clone()
        ),
        (
            json!("success"),
            json!("main"),
            json!("2026-10-01T10:00:00.123000+00:00"),
            json!("https://gl/p/12")
        )
    );
    let repos = app
        .get(&sa.token, &format!("{R}/gitlab/repos"))
        .await
        .json();
    let row = repos
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(repo))
        .unwrap()
        .clone();
    assert_eq!(
        (
            row["last_pipeline_status"].clone(),
            row["last_pipeline_ref"].clone()
        ),
        (json!("success"), json!("main"))
    );
}

#[tokio::test]
async fn sync_pipelines_errors_are_502() {
    let app = app();
    let sa = app.superadmin().await;
    granary_services::safe_fetch::allow_loopback_for_tests();
    let gl = MockServer::start().await;
    let (_, repo) = config_and_repo(app, &sa.token, &gl.uri(), 78).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/78/pipelines"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&gl)
        .await;
    let url = format!("{R}/gitlab/repos/{repo}/sync-pipelines");
    let res = app.post(&sa.token, &url, json!({})).await;
    assert_eq!(
        (res.status, res.json()),
        (502, json!({"detail": "GitLab API returned 500"}))
    );
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/78/pipelines"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"status": "x"}])))
        .mount(&gl)
        .await;
    let res = app.post(&sa.token, &url, json!({})).await;
    assert_eq!((res.status, res.json()), (502, json!({"detail": "'id'"})));
    let missing = Uuid::new_v4();
    let res = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/repos/{missing}/sync-pipelines"),
            json!({}),
        )
        .await;
    assert_eq!(
        res.json(),
        json!({"detail": format!("GitLabRepository '{missing}' not found"), "code": "NOT_FOUND"})
    );
}

#[tokio::test]
async fn the_webhook_updates_a_known_pipeline_run() {
    let app = app();
    let sa = app.superadmin().await;
    let base = format!("https://{}.gitlab.test", u("gl"));
    let (_, repo) = config_and_repo(app, &sa.token, &base, 4242).await;
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO gitlab_pipeline_runs (id, repository_id, gitlab_pipeline_id, status) VALUES ($1, $2::uuid, 900, 'running')",
    )
    .bind(Uuid::new_v4())
    .bind(&repo)
    .execute(&mut db)
    .await
    .unwrap();
    let hook = json!({"project": {"web_url": format!("{base}/acme/face-api"), "id": 4242},
                      "user": {"name": "Dev", "username": "dev"},
                      "object_attributes": {"id": 900, "status": "failed", "ref": "main",
                                            "finished_at": "2026-10-02T03:04:05Z", "duration": 61}});
    let rb = app
        .client()
        .post(app.url(&format!("{R}/gitlab/webhook")))
        .header("X-Gitlab-Token", "hook")
        .header("X-Gitlab-Event", "Pipeline Hook")
        .json(&hook);
    let res = app.send(rb).await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(
        res.json(),
        json!({"accepted": true, "event_type": "pipeline"})
    );
    let (status, fin, dur): (String, Option<chrono::DateTime<chrono::Utc>>, Option<i32>) = sqlx::query_as(
        "SELECT status, finished_at, duration_seconds FROM gitlab_pipeline_runs WHERE repository_id = $1::uuid AND gitlab_pipeline_id = 900",
    )
    .bind(&repo)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(fin.unwrap().to_rfc3339(), "2026-10-02T03:04:05+00:00");
    assert_eq!(dur, Some(61));
    let raw: Value = sqlx::query_scalar(
        "SELECT raw_payload FROM gitlab_webhook_events WHERE repository_id = $1::uuid",
    )
    .bind(&repo)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(raw, hook);
    // Not JSON: request.json() raises, the 500 handler answers.
    let rb = app
        .client()
        .post(app.url(&format!("{R}/gitlab/webhook")))
        .body("nope");
    assert_eq!(app.send(rb).await.status, 500);
}

#[tokio::test]
async fn patch_echoes_the_values_python_set() {
    let app = app();
    let sa = app.superadmin().await;
    let p = app
        .post(
            &sa.token,
            &format!("{R}/projects"),
            json!({"code": u("rnd"), "name": "Echo"}),
        )
        .await
        .json();
    // A UUID string is returned as given (SQLAlchemy keeps the str on the object).
    let lead = sa.id.to_string().to_uppercase();
    let res = app
        .patch(
            &sa.token,
            &format!("{R}/projects/{}", p["id"].as_str().unwrap()),
            json!({"lead_engineer_id": lead}),
        )
        .await;
    assert_eq!(res.json()["lead_engineer_id"], json!(lead));
    // A str on an integer column is asyncpg's DataError: a 500.
    let s = app
        .post(
            &sa.token,
            &format!("{R}/projects/{}/sprints", p["id"].as_str().unwrap()),
            json!({"name": "S"}),
        )
        .await
        .json();
    let res = app
        .patch(
            &sa.token,
            &format!("{R}/sprints/{}", s["id"].as_str().unwrap()),
            json!({"velocity_planned": "5"}),
        )
        .await;
    assert_eq!(res.status, 500);
    // true on an integer column is written as 1 and echoed as true.
    let res = app
        .patch(
            &sa.token,
            &format!("{R}/sprints/{}", s["id"].as_str().unwrap()),
            json!({"velocity_planned": true}),
        )
        .await;
    assert_eq!(res.json()["velocity_planned"], json!(true));
    assert_eq!(
        app.get(&sa.token, &format!("{R}/projects/nope/sprints"))
            .await
            .status,
        422
    );
}

/// Security triage #21: the config URL is typed in by staff; an internal address (cloud metadata, a private
/// network, a loopback name) is refused before any request is sent, and the test reports it.
#[tokio::test]
async fn test_connection_refuses_internal_addresses() {
    let app = app();
    let sa = app.superadmin().await;
    for url in [
        "http://169.254.169.254",
        "http://10.0.0.7",
        "http://192.168.1.1",
        "http://[::ffff:10.0.0.1]",
    ] {
        let (cfg, _) = config_and_repo(app, &sa.token, url, 3).await;
        let res = app
            .post(
                &sa.token,
                &format!("{R}/gitlab/configs/{cfg}/test"),
                json!({}),
            )
            .await;
        assert_eq!(res.status, 200, "{}", res.text());
        let body = res.json();
        assert_eq!(body["connected"], json!(false), "{url}: {body}");
        let err = body["error"].as_str().unwrap_or_default();
        assert!(
            err.starts_with("Refused:") && err.contains("private or internal"),
            "{url}: {err}"
        );
    }
}

/// 17 Nov 2026: the server's projects for Add repository (the token's projects, search passed on, linked marked);
/// a refused token reads as a clear 502; the config list says whether a webhook secret is set.
#[tokio::test]
async fn the_server_lists_its_projects_to_pick_from() {
    let app = app();
    let sa = app.superadmin().await;
    granary_services::safe_fetch::allow_loopback_for_tests();
    let gl = MockServer::start().await;
    let (cfg, _) = config_and_repo(app, &sa.token, &gl.uri(), 77).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects"))
        .and(query_param("membership", "true"))
        .and(query_param("search", "face"))
        .and(header("PRIVATE-TOKEN", "glpat-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 77, "name": "face-api", "path_with_namespace": "acme/face-api", "web_url": "https://gl/acme/face-api",
             "default_branch": "main", "description": null, "last_activity_at": "2026-11-16T10:00:00Z", "owner": {"x": 1}},
            {"id": 78, "name": "SDK", "path_with_namespace": "products/sdk", "default_branch": "develop"},
        ])))
        .mount(&gl)
        .await;
    let r = app
        .get(
            &sa.token,
            &format!("{R}/gitlab/configs/{cfg}/projects?search=face"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let list = r.json();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], 77);
    assert_eq!(list[0]["linked"], true, "already linked");
    assert_eq!(list[1]["linked"], false);
    assert_eq!(list[1]["default_branch"], "develop");
    assert!(
        list[0].get("owner").is_none(),
        "only the fields the page needs"
    );

    Mock::given(method("GET"))
        .and(path("/api/v4/projects"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&gl)
        .await;
    let r = app
        .get(&sa.token, &format!("{R}/gitlab/configs/{cfg}/projects"))
        .await;
    assert_eq!(r.status, 502);
    assert!(
        r.text().contains("refused the access token"),
        "{}",
        r.text()
    );

    let configs = app
        .get(&sa.token, &format!("{R}/gitlab/configs"))
        .await
        .json();
    let row = configs
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == cfg.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["has_webhook_secret"], true);
    assert!(
        row.get("webhook_secret").is_none(),
        "the secret itself is never listed"
    );
}

/// The /rnd/projects list rows carry `backlog_counts` (per status, live items only) like the detail does, so
/// the "Items done" column and the "Total items" tile are not 0 while the boards have items.
#[tokio::test]
async fn project_list_carries_backlog_counts() {
    let app = app();
    let sa = app.superadmin().await;
    let res = app
        .post(
            &sa.token,
            &format!("{R}/projects"),
            json!({"code": u("CNT").replace('-', ""), "name": "Counts", "product_ref": "portal"}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let pid = res.json()["id"].as_str().unwrap().to_string();
    let mut ids = Vec::new();
    for title in ["a", "b", "c"] {
        let r = app
            .post(
                &sa.token,
                &format!("{R}/backlog"),
                json!({"rnd_project_id": pid, "title": title}),
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.text());
        ids.push(Uuid::parse_str(r.json()["id"].as_str().unwrap()).unwrap());
    }
    let mut db = app.db().await;
    sqlx::query("UPDATE rnd_backlog_items SET status = 'done' WHERE id = $1")
        .bind(ids[0])
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE rnd_backlog_items SET is_deleted = true, deleted_at = now() WHERE id = $1")
        .bind(ids[2])
        .execute(&mut db)
        .await
        .unwrap();
    let detail = app
        .get(&sa.token, &format!("{R}/projects/{pid}"))
        .await
        .json();
    let list = app.get(&sa.token, &format!("{R}/projects?limit=500")).await;
    assert_eq!(list.status, 200, "{}", list.text());
    let row = list
        .json()
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(pid))
        .cloned()
        .expect("the project is listed");
    let counts = &row["backlog_counts"];
    let total: i64 = counts
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_i64().unwrap())
        .sum();
    assert_eq!(total, 2, "{counts}");
    assert_eq!(counts["done"], json!(1));
    assert_eq!(row["backlog_counts"], detail["backlog_counts"]);
    assert_eq!(row["product_ref"], json!("portal"));
}

/// Batch E: product_ref is stored trimmed and lower case on create and update (product-line codes are lower
/// case), and the list filter matches it whatever the case asked.
#[tokio::test]
async fn product_ref_is_stored_lower_case() {
    let app = app();
    let sa = app.superadmin().await;
    let tag = u("Line").to_uppercase();
    let p = app
        .post(
            &sa.token,
            &format!("{R}/projects"),
            json!({"code": u("rnd"), "name": "Ref", "product_ref": format!("  {tag} ")}),
        )
        .await;
    assert_eq!(p.status, 201, "{}", p.text());
    let p = p.json();
    assert_eq!(p["product_ref"], json!(tag.to_lowercase()));
    let id = p["id"].as_str().unwrap();
    let res = app
        .patch(
            &sa.token,
            &format!("{R}/projects/{id}"),
            json!({"product_ref": format!("{tag}-Two")}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let want = format!("{tag}-Two").to_lowercase();
    assert_eq!(res.json()["product_ref"], json!(want));
    let listed = app
        .get(&sa.token, &format!("{R}/projects?product_ref={tag}-TWO"))
        .await
        .json();
    let rows = listed.get("items").cloned().unwrap_or(listed.clone());
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == json!(id)),
        "{listed}"
    );
    // Blank clears it.
    let res = app
        .patch(
            &sa.token,
            &format!("{R}/projects/{id}"),
            json!({"product_ref": "  "}),
        )
        .await;
    assert_eq!(res.json()["product_ref"], Value::Null);
}
