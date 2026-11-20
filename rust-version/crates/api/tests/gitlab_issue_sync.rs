//! 17 Nov 2026 (Owner): R&D backlog items <-> GitLab issues, two-way (services/rnd_gitlab.rs, migration 0179),
//! against a fake GitLab that keeps its issues and notes:
//! - turning the sync on imports both ways and creates the scoped labels; the Owner is told;
//! - an ERP edit reaches the issue; a GitLab edit (webhook) reaches the item, with a history line;
//! - both sides changed: the newer edit wins, the loser is in the history;
//! - comments both ways, our own coming back never doubles;
//! - an item deleted in the ERP closes its issue with erp::deleted; an issue deleted in GitLab unlinks the item;
//! - a GitLab refusal is kept on the item and tried again later.

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use support::{u, TestApp};
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const R: &str = "/api/v1/rnd";
const PID: i64 = 77;
const SECRET: &str = "hook-secret-1";

fn app() -> &'static TestApp {
    support::app("gitlab_sync")
}

#[derive(Default)]
struct State {
    issues: BTreeMap<i64, Value>,
    notes: Vec<(i64, Value)>,
    labels: Vec<String>,
    next_note: i64,
    /// answer every issue write with this status (a refusal)
    refuse_writes: Option<u16>,
}

#[derive(Clone)]
struct Fake(Arc<Mutex<State>>);

fn labels_of(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => vec![],
    }
}

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let path = req.url.path().to_string();
        let base = format!("/api/v4/projects/{PID}");
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let now = Utc::now().to_rfc3339();
        let m = req.method.as_str();
        if path == "/api/v4/users" {
            return ResponseTemplate::new(200).set_body_json(json!([]));
        }
        if path.starts_with("/api/v4/users/") {
            return ResponseTemplate::new(404).set_body_json(json!({"message": "404 Not found"}));
        }
        if path == format!("{base}/labels") {
            let name = body["name"].as_str().unwrap_or("").to_string();
            if st.labels.contains(&name) {
                return ResponseTemplate::new(409)
                    .set_body_json(json!({"message": "Label already exists"}));
            }
            st.labels.push(name);
            return ResponseTemplate::new(201).set_body_json(json!({}));
        }
        if path == format!("{base}/issues") {
            if m == "GET" {
                let all: Vec<Value> = st.issues.values().cloned().collect();
                return ResponseTemplate::new(200).set_body_json(all);
            }
            if let Some(code) = st.refuse_writes {
                return ResponseTemplate::new(code).set_body_json(json!({"message": "refused"}));
            }
            let iid = st.issues.keys().max().copied().unwrap_or(0) + 1;
            let issue = json!({
                "id": 5000 + iid, "iid": iid, "title": body["title"], "description": body["description"],
                "state": "opened", "labels": labels_of(&body["labels"]), "assignees": [],
                "created_at": now, "updated_at": now,
                "web_url": format!("https://gl.example/p/-/issues/{iid}"), "author": {"id": 1},
            });
            st.issues.insert(iid, issue.clone());
            return ResponseTemplate::new(201).set_body_json(issue);
        }
        let rest = path.strip_prefix(&format!("{base}/issues/")).unwrap_or("");
        let mut parts = rest.split('/');
        let iid: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let tail = parts.next();
        if tail == Some("notes") {
            st.next_note += 1;
            let note = json!({"id": 900 + st.next_note, "body": body["body"]});
            st.notes.push((iid, note.clone()));
            return ResponseTemplate::new(201).set_body_json(note);
        }
        if m == "PUT" {
            if let Some(code) = st.refuse_writes {
                return ResponseTemplate::new(code).set_body_json(json!({"message": "refused"}));
            }
        }
        let Some(issue) = st.issues.get_mut(&iid) else {
            return ResponseTemplate::new(404)
                .set_body_json(json!({"message": "404 Issue Not Found"}));
        };
        if m == "PUT" {
            for k in ["title", "description"] {
                if let Some(v) = body.get(k) {
                    issue[k] = v.clone();
                }
            }
            if let Some(l) = body.get("labels") {
                issue["labels"] = json!(labels_of(l));
            }
            if let Some(l) = body.get("add_labels") {
                let mut cur = labels_of(&issue["labels"]);
                cur.extend(labels_of(l));
                issue["labels"] = json!(cur);
            }
            match body["state_event"].as_str() {
                Some("close") => issue["state"] = json!("closed"),
                Some("reopen") => issue["state"] = json!("opened"),
                _ => {}
            }
            if let Some(a) = body.get("assignee_ids") {
                issue["assignees"] = json!(a
                    .as_array()
                    .map(|x| x
                        .iter()
                        .map(|id| json!({"id": id, "username": "x"}))
                        .collect::<Vec<_>>())
                    .unwrap_or_default());
            }
            issue["updated_at"] = json!(now);
        }
        ResponseTemplate::new(200).set_body_json(issue.clone())
    }
}

async fn item(app: &TestApp, id: &str) -> Value {
    let sa = app.superadmin().await;
    let r = app.get(&sa.token, &format!("{R}/backlog/{id}")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

async fn sync(app: &TestApp, token: &str, repo: &str) -> Value {
    let r = app
        .post(
            token,
            &format!("{R}/gitlab/repos/{repo}/sync-issues"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

async fn webhook(app: &TestApp, url: &str, event: &str, payload: Value) {
    let mut p = payload;
    p["project"] = json!({"id": PID, "web_url": format!("{url}/acme/face-api")});
    let r = app
        .send(
            app.client()
                .post(app.url(&format!("{R}/gitlab/webhook")))
                .header("content-type", "application/json")
                .header("x-gitlab-token", SECRET)
                .header("x-gitlab-event", event)
                .body(serde_json::to_vec(&p).unwrap()),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

async fn history(app: &TestApp, item: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let mut db = app.db().await;
    sqlx::query_as(
        "SELECT verb, field, new_value FROM issue_activity WHERE entity_type = 'rnd_backlog_item' AND entity_id = $1 \
         ORDER BY created_at",
    )
    .bind(item)
    .fetch_all(&mut db)
    .await
    .unwrap()
}

#[tokio::test]
async fn backlog_items_and_issues_sync_both_ways() {
    let app = app();
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    granary_services::safe_fetch::allow_loopback_for_tests();
    let gl = MockServer::start().await;
    let state = Arc::new(Mutex::new(State::default()));
    Mock::given(wiremock::matchers::any())
        .respond_with(Fake(state.clone()))
        .mount(&gl)
        .await;
    // a GitLab issue that exists before the link
    {
        let mut st = state.lock().unwrap();
        st.issues.insert(
            1,
            json!({"id": 5001, "iid": 1, "title": "Printer reconnect", "description": "Reconnect after loss.\n\n## Acceptance criteria\n\nWithin 5 s",
                   "state": "opened", "labels": ["status::in_progress", "priority::high", "ux"], "assignees": [],
                   "created_at": "2026-11-10T10:00:00Z", "updated_at": "2026-11-10T10:00:00Z",
                   "web_url": "https://gl.example/p/-/issues/1", "author": {"id": 1}}),
        );
    }
    // the R&D project, its items, the server and the repository
    let p = app
        .post(
            &sa.token,
            &format!("{R}/projects"),
            json!({"code": u("FS"), "name": u("SDK")}),
        )
        .await;
    assert_eq!(p.status, 201, "{}", p.text());
    let pid = p.json()["id"].as_str().unwrap().to_string();
    let mk = |title: &str| json!({"rnd_project_id": pid, "title": title, "priority": "low", "item_type": "bug"});
    let open = app
        .post(
            &sa.token,
            &format!("{R}/backlog"),
            mk("Crash on empty frame"),
        )
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let done = app
        .post(&sa.token, &format!("{R}/backlog"), mk("Old fix"))
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = app
        .patch(
            &sa.token,
            &format!("{R}/backlog/{done}"),
            json!({"status": "done"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let cfg = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/configs"),
            json!({"name": u("GL"), "url": format!("{}/", gl.uri()), "access_token": "glpat-api", "webhook_secret": SECRET}),
        )
        .await;
    assert_eq!(cfg.status, 201, "{}", cfg.text());
    let cfg_id = cfg.json()["id"].as_str().unwrap().to_string();
    let repo = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/repos"),
            json!({"gitlab_config_id": cfg_id, "gitlab_project_id": PID, "full_path": "acme/face-api",
                   "name": "face-api", "rnd_project_id": pid}),
        )
        .await;
    assert_eq!(repo.status, 201, "{}", repo.text());
    let repo = repo.json()["id"].as_str().unwrap().to_string();

    // off by default: no sync
    let r = app
        .post(
            &sa.token,
            &format!("{R}/gitlab/repos/{repo}/sync-issues"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());

    // on: the first sync imports both ways
    let r = app
        .patch(
            &sa.token,
            &format!("{R}/gitlab/repos/{repo}"),
            json!({"issue_sync": true}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = sync(app, &sa.token, &repo).await;
    assert_eq!(out["imported"], 1, "{out}");
    assert_eq!(out["created"], 2, "{out}");
    {
        let st = state.lock().unwrap();
        assert!(
            st.labels.contains(&"status::in_progress".to_string())
                && st.labels.contains(&"erp::deleted".to_string())
        );
        assert_eq!(st.issues.len(), 3);
        let crash = st
            .issues
            .values()
            .find(|i| i["title"] == "Crash on empty frame")
            .unwrap();
        assert_eq!(crash["state"], "opened");
        assert_eq!(
            labels_of(&crash["labels"]),
            vec!["status::backlog", "priority::low", "type::bug"]
        );
        let old = st
            .issues
            .values()
            .find(|i| i["title"] == "Old fix")
            .unwrap();
        assert_eq!(old["state"], "closed", "a done item's issue is closed");
    }
    let mut db = app.db().await;
    let imported: (String, String, String, Option<String>, Value) = sqlx::query_as(
        "SELECT id::text, status, priority, acceptance_criteria, labels FROM rnd_backlog_items \
         WHERE gitlab_repository_id = $1::uuid AND gitlab_issue_iid = 1",
    )
    .bind(&repo)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (imported.1.as_str(), imported.2.as_str()),
        ("in_progress", "high")
    );
    assert_eq!(imported.3.as_deref(), Some("Within 5 s"));
    assert_eq!(imported.4, json!(["ux"]));
    let imp = imported.0.clone();
    let told: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE type = 'gitlab_sync'")
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(told >= 1, "the Owner is told");

    // a quiet sync changes nothing
    let out = sync(app, &sa.token, &repo).await;
    assert_eq!(out["pushed"], 0, "{out}");

    // an ERP edit reaches the issue
    let crash_iid = item(app, &open).await["gitlab_issue_iid"].as_i64().unwrap();
    app.patch(
        &sa.token,
        &format!("{R}/backlog/{open}"),
        json!({"title": "Crash on an empty frame", "status": "in_progress"}),
    )
    .await;
    let out = sync(app, &sa.token, &repo).await;
    assert_eq!(out["pushed"], 1, "{out}");
    {
        let st = state.lock().unwrap();
        let i = &st.issues[&crash_iid];
        assert_eq!(i["title"], "Crash on an empty frame");
        assert!(labels_of(&i["labels"]).contains(&"status::in_progress".to_string()));
    }

    // a GitLab edit (webhook) reaches the item, with a history line
    {
        let mut st = state.lock().unwrap();
        let i = st.issues.get_mut(&1).unwrap();
        i["title"] = json!("Printer reconnects by itself");
        i["labels"] = json!(["status::review", "priority::high", "ux"]);
        i["updated_at"] = json!(Utc::now().to_rfc3339());
    }
    webhook(
        app,
        &gl.uri(),
        "Issue Hook",
        json!({"object_kind": "issue", "object_attributes": {"id": 5001, "iid": 1}}),
    )
    .await;
    sync(app, &sa.token, &repo).await;
    let it = item(app, &imp).await;
    assert_eq!(
        (it["title"].as_str(), it["status"].as_str()),
        (Some("Printer reconnects by itself"), Some("review"))
    );
    let h = history(app, &imp).await;
    assert!(
        h.iter()
            .any(|(v, f, _)| v == "gitlab" && f.as_deref() == Some("status")),
        "{h:?}"
    );
    let out = sync(app, &sa.token, &repo).await;
    assert_eq!(out["pushed"], 0, "our own pull is not pushed back: {out}");

    // both sides changed the status: the newer (ERP) edit wins, GitLab's value goes to the history
    {
        let mut st = state.lock().unwrap();
        let i = st.issues.get_mut(&1).unwrap();
        i["state"] = json!("closed");
        i["updated_at"] = json!((Utc::now() - Duration::minutes(5)).to_rfc3339());
    }
    app.patch(
        &sa.token,
        &format!("{R}/backlog/{imp}"),
        json!({"status": "in_progress"}),
    )
    .await;
    sync(app, &sa.token, &repo).await;
    assert_eq!(item(app, &imp).await["status"], "in_progress");
    {
        let st = state.lock().unwrap();
        assert_eq!(
            st.issues[&1]["state"], "opened",
            "reopened by the newer ERP edit"
        );
    }
    let h = history(app, &imp).await;
    assert!(
        h.iter()
            .any(|(v, f, _)| v == "gitlab_overwritten" && f.as_deref() == Some("status")),
        "{h:?}"
    );

    // comments: ERP -> GitLab with the author's name; the webhook echo links it, never a second comment
    let c = app
        .post(
            &sa.token,
            "/api/v1/collab/comments",
            json!({"entity_type": "rnd_backlog_item", "entity_id": imp, "body": "Tested on site"}),
        )
        .await;
    assert_eq!(c.status, 201, "{}", c.text());
    let out = sync(app, &sa.token, &repo).await;
    assert_eq!(out["comments"], 1, "{out}");
    let (note_id, note_body) = {
        let st = state.lock().unwrap();
        let (_, n) = st.notes.last().unwrap().clone();
        (
            n["id"].as_i64().unwrap(),
            n["body"].as_str().unwrap().to_string(),
        )
    };
    assert!(
        note_body.contains("(ERP):**") && note_body.contains("Tested on site"),
        "{note_body}"
    );
    webhook(
        app,
        &gl.uri(),
        "Note Hook",
        json!({"object_kind": "note", "user": {"id": 1, "username": "bot"}, "issue": {"iid": 1},
               "object_attributes": {"id": note_id, "note": note_body, "noteable_type": "Issue", "system": false}}),
    )
    .await;
    // a GitLab user's comment becomes an ERP comment (no ERP user matched: the GitLab username is shown)
    webhook(
        app,
        &gl.uri(),
        "Note Hook",
        json!({"object_kind": "note", "user": {"id": 42, "username": "dina"}, "issue": {"iid": 1},
               "object_attributes": {"id": 7777, "note": "Fixed in 2.1.0", "noteable_type": "Issue", "system": false}}),
    )
    .await;
    let comments: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT body, origin FROM issue_comments WHERE entity_type = 'rnd_backlog_item' AND entity_id = $1 ORDER BY created_at",
    )
    .bind(&imp)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(comments.len(), 2, "{comments:?}");
    assert_eq!(
        comments[1],
        (
            "@dina (GitLab): Fixed in 2.1.0".to_string(),
            Some("gitlab".to_string())
        )
    );
    let out = sync(app, &sa.token, &repo).await;
    assert_eq!(
        out["comments"], 0,
        "a GitLab comment is never sent back: {out}"
    );

    // a GitLab refusal is kept on the item and tried later
    state.lock().unwrap().refuse_writes = Some(403);
    app.patch(
        &sa.token,
        &format!("{R}/backlog/{open}"),
        json!({"priority": "critical"}),
    )
    .await;
    let out = sync(app, &sa.token, &repo).await;
    assert_eq!(out["errors"], 1, "{out}");
    let err: Option<String> =
        sqlx::query_scalar("SELECT gitlab_sync_error FROM rnd_backlog_items WHERE id = $1::uuid")
            .bind(&open)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(
        err.unwrap_or_default().contains("api scope"),
        "a clear reason"
    );
    state.lock().unwrap().refuse_writes = None;
    sqlx::query("UPDATE rnd_backlog_items SET gitlab_retry_at = now() WHERE id = $1::uuid")
        .bind(&open)
        .execute(&mut db)
        .await
        .unwrap();
    sync(app, &sa.token, &repo).await;
    assert!(
        labels_of(&state.lock().unwrap().issues[&crash_iid]["labels"])
            .contains(&"priority::critical".to_string())
    );

    // deleted in the ERP: the issue is closed with erp::deleted (never deleted)
    let r = app.delete(&sa.token, &format!("{R}/backlog/{open}")).await;
    assert_eq!(r.status, 204, "{}", r.text());
    sync(app, &sa.token, &repo).await;
    {
        let st = state.lock().unwrap();
        let i = &st.issues[&crash_iid];
        assert_eq!(i["state"], "closed");
        assert!(labels_of(&i["labels"]).contains(&"erp::deleted".to_string()));
    }

    // deleted in GitLab: the item stays, unlinked, with a history line
    state.lock().unwrap().issues.remove(&1);
    app.patch(
        &sa.token,
        &format!("{R}/backlog/{imp}"),
        json!({"priority": "low"}),
    )
    .await;
    sync(app, &sa.token, &repo).await;
    let it = item(app, &imp).await;
    assert!(it["gitlab_issue_iid"].is_null(), "{it}");
    let h = history(app, &imp).await;
    assert!(
        h.iter()
            .any(|(_, _, n)| n.as_deref().unwrap_or("").contains("was deleted")),
        "{h:?}"
    );

    // the repository list shows the sync state
    let repos = app
        .get(&sa.token, &format!("{R}/gitlab/repos?config_id={cfg_id}"))
        .await
        .json();
    assert_eq!(repos[0]["issue_sync"], true);
    assert!(repos[0]["issue_sync_at"].is_string());
    let people = app
        .get(&sa.token, &format!("{R}/gitlab/configs/{cfg_id}/people"))
        .await;
    assert_eq!(people.status, 200, "{}", people.text());
    let _ = Uuid::nil();
}
