//! R&D backlog items <-> GitLab issues, two-way (17 Nov 2026, Owner; migration 0179).
//!
//! Per repository (`gitlab_repositories.issue_sync`, off until turned on; the repository must belong to an R&D
//! project). The worker's tick (every minute) per repository:
//! 1. once: the scoped labels (`status::*`, `priority::*`, `type::*`, `erp::deleted`) in the GitLab project;
//! 2. once, at the first sync: import both ways: every GitLab issue becomes a backlog item, every backlog item of
//!    the R&D project without an issue gets one (a done item's issue is created closed); the Owner is told;
//! 3. the issues GitLab told about (webhook -> `gitlab_issue_queue`);
//! 4. every `rnd.gitlab.pull_minutes`: the issues updated since the last pull (a missed webhook is caught);
//! 5. the items changed in the ERP since their last sync (any write path: the editor, sprint start, Nary);
//! 6. the ERP comments not yet in GitLab (posted by the token's account as "**Name (ERP):**").
//!
//! GitLab comments come in through the webhook (`note_event`), by the matched ERP user or the GitLab username.
//!
//! Merging: each linked item keeps both sides' values at the last sync (`gitlab_snapshot {"erp", "gl"}`). A field
//! changed on one side only goes to the other; changed on both: the newer edit wins (the item's updated_at vs the
//! issue's) and the losing value is written to the item's history. Our own writes coming back match the snapshot,
//! so nothing loops.
//!
//! Mapping: title; description + "## Acceptance criteria" section; status: done (and released) close the issue,
//! the open statuses are a `status::*` label; priority and item type as `priority::*` / `type::*`; the item's own
//! labels as plain labels; the assignee through `gitlab_user_links` (by e-mail, or set by hand). Story points,
//! sprint, epic and board rank stay ERP-only. An item deleted in the ERP closes its issue with `erp::deleted`; an
//! issue deleted in GitLab leaves the item, unlinked, with a history line.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use granary_compat::pyjson::PyValue;
use granary_core::settings_store::require_int;
use granary_core::ApiResult;
use reqwest::Method;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

pub const BATCH_KEY: &str = "rnd.gitlab.sync_batch";
pub const PULL_KEY: &str = "rnd.gitlab.pull_minutes";
pub const RETRY_KEY: &str = "rnd.gitlab.retry_max_minutes";
const WHERE: &str = "Integrations & access \u{2192} GitLab";

pub const OPEN_STATUSES: &[&str] = &["backlog", "refined", "in_sprint", "in_progress", "review"];
const CLOSED_STATUSES: &[&str] = &["done", "released"];
pub const PRIORITIES: &[&str] = &["critical", "high", "medium", "low"];
pub const TYPES: &[&str] = &[
    "feature",
    "bug",
    "tech_debt",
    "research",
    "spike",
    "security",
    "documentation",
    "internal_tool",
];
pub const DELETED_LABEL: &str = "erp::deleted";
const AC_HEADER: &str = "## Acceptance criteria";
const ENTITY: &str = "rnd_backlog_item";
const FIELDS: &[&str] = &[
    "title",
    "description",
    "status",
    "priority",
    "item_type",
    "labels",
    "assignee",
];

fn is_closed(status: &str) -> bool {
    CLOSED_STATUSES.contains(&status)
}

// ─── the repositories ────────────────────────────────────────────────────────

/// A repository with issue sync on, with its server's API base and token.
#[derive(Debug, Clone)]
pub struct Repo {
    pub id: Uuid,
    pub config_id: Uuid,
    pub project_id: i64,
    pub rnd_project_id: Uuid,
    pub api: String,
    pub token: String,
    pub imported_at: Option<DateTime<Utc>>,
    pub labels_at: Option<DateTime<Utc>>,
    pub pulled_at: Option<DateTime<Utc>>,
}

/// The repositories whose issues sync (one when `only` is given).
pub async fn repos(db: &mut PgConnection, only: Option<Uuid>) -> ApiResult<Vec<Repo>> {
    let rows = sqlx::query(
        "SELECT r.id, r.gitlab_config_id, r.gitlab_project_id::bigint AS pid, r.rnd_project_id, c.url, c.api_version, \
         c.access_token_encrypted, r.issue_sync_imported_at, r.issue_sync_labels_at, r.issue_sync_pulled_at \
         FROM gitlab_repositories r JOIN gitlab_configs c ON c.id = r.gitlab_config_id \
         WHERE r.issue_sync AND r.is_active AND c.is_active AND r.rnd_project_id IS NOT NULL \
         AND ($1::uuid IS NULL OR r.id = $1) ORDER BY r.created_at",
    )
    .bind(only)
    .fetch_all(&mut *db)
    .await?;
    let key = granary_core::config::settings().APP_SECRET_KEY.clone();
    let mut out = vec![];
    for r in rows {
        let id: Uuid = r.get("id");
        let enc: Option<String> = r.get("access_token_encrypted");
        let token = match enc
            .filter(|t| !t.is_empty())
            .map(|t| granary_compat::crypto::fernet::decrypt(&key, &t))
        {
            Some(Ok(t)) => t,
            _ => {
                repo_error(db, id, "the GitLab server has no readable access token").await?;
                continue;
            }
        };
        let url: String = r.get("url");
        let ver: Option<String> = r.get("api_version");
        out.push(Repo {
            id,
            config_id: r.get("gitlab_config_id"),
            project_id: r.get("pid"),
            rnd_project_id: r.get("rnd_project_id"),
            api: format!(
                "{}/api/{}",
                url.trim_end_matches('/'),
                ver.unwrap_or_else(|| "v4".into())
            ),
            token,
            imported_at: r.get("issue_sync_imported_at"),
            labels_at: r.get("issue_sync_labels_at"),
            pulled_at: r.get("issue_sync_pulled_at"),
        });
    }
    Ok(out)
}

async fn repo_error(db: &mut PgConnection, id: Uuid, why: &str) -> ApiResult<()> {
    sqlx::query(
        "UPDATE gitlab_repositories SET issue_sync_error = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(why.chars().take(1000).collect::<String>())
    .execute(&mut *db)
    .await?;
    Ok(())
}

// ─── GitLab calls ────────────────────────────────────────────────────────────

/// One GitLab API answer: status, JSON body (Null when empty), and the next page (X-Next-Page).
struct Answer {
    status: u16,
    body: Value,
    next_page: Option<String>,
}

async fn call(
    repo: &Repo,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Answer, String> {
    let url = format!("{}{path}", repo.api);
    let resp = crate::safe_fetch::request(
        method,
        &url,
        &[("PRIVATE-TOKEN", repo.token.as_str())],
        query,
        body,
        Duration::from_secs(20),
    )
    .await
    .map_err(|e| format!("GitLab could not be reached: {e}"))?;
    let status = resp.status().as_u16();
    let next_page = resp
        .headers()
        .get("x-next-page")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let text = resp
        .text()
        .await
        .map_err(|e| format!("GitLab's answer could not be read: {e}"))?;
    let body = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text.chars().take(300).collect()))
    };
    Ok(Answer {
        status,
        body,
        next_page,
    })
}

/// A 2xx answer's body, else the reason.
async fn ok(
    repo: &Repo,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let a = call(repo, method, path, query, body).await?;
    if (200..300).contains(&a.status) {
        return Ok(a.body);
    }
    Err(refusal(a.status, &a.body))
}

fn refusal(status: u16, body: &Value) -> String {
    let msg = body
        .get("message")
        .or_else(|| body.get("error"))
        .map(|m| match m {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    match status {
        401 => "GitLab refused the access token (expired or revoked)".into(),
        403 => format!(
            "GitLab refused the write: the access token needs the api scope and a Reporter role or higher ({msg})"
        ),
        _ => format!("GitLab answered {status}{}", if msg.is_empty() { String::new() } else { format!(": {msg}") }),
    }
}

fn issue_path(repo: &Repo, iid: i64) -> String {
    format!("/projects/{}/issues/{iid}", repo.project_id)
}

/// Every page of a list call (at most `max_pages` of 100).
async fn all_pages(
    repo: &Repo,
    path: &str,
    query: &[(&str, &str)],
    max_pages: usize,
) -> Result<Vec<Value>, String> {
    let mut out = vec![];
    let mut page = "1".to_string();
    for _ in 0..max_pages {
        let mut q: Vec<(&str, &str)> = query.to_vec();
        q.push(("per_page", "100"));
        q.push(("page", page.as_str()));
        let a = call(repo, Method::GET, path, &q, None).await?;
        if !(200..300).contains(&a.status) {
            return Err(refusal(a.status, &a.body));
        }
        if let Value::Array(items) = a.body {
            out.extend(items);
        }
        match a.next_page {
            Some(n) => page = n,
            None => break,
        }
    }
    Ok(out)
}

// ─── mapping ─────────────────────────────────────────────────────────────────

/// The description GitLab shows: the item's description and its acceptance criteria as a section.
pub fn compose(description: Option<&str>, criteria: Option<&str>) -> String {
    let d = description.unwrap_or("").trim();
    let c = criteria.unwrap_or("").trim();
    match (d.is_empty(), c.is_empty()) {
        (_, true) => d.to_string(),
        (true, false) => format!("{AC_HEADER}\n\n{c}"),
        (false, false) => format!("{d}\n\n{AC_HEADER}\n\n{c}"),
    }
}

/// The reverse of [`compose`]: (description, acceptance criteria).
pub fn split(text: &str) -> (Option<String>, Option<String>) {
    let lower = text.to_lowercase();
    let at = if lower.starts_with(&AC_HEADER.to_lowercase()) {
        Some(0)
    } else {
        lower
            .find(&format!("\n{}", AC_HEADER.to_lowercase()))
            .map(|i| i + 1)
    };
    let none_if_empty = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    match at {
        Some(i) => (
            none_if_empty(&text[..i]),
            none_if_empty(&text[i + AC_HEADER.len()..]),
        ),
        None => (none_if_empty(text), None),
    }
}

fn plain_labels(v: &Value) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    if let Value::Array(a) = v {
        for l in a.iter().filter_map(Value::as_str) {
            let l = l.trim();
            if !l.is_empty() && !l.contains("::") {
                set.insert(l.to_string());
            }
        }
    }
    set.into_iter().collect()
}

/// A backlog item as the sync reads it.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub criteria: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub item_type: Option<String>,
    pub labels: Value,
    pub assignee_id: Option<Uuid>,
    pub is_deleted: bool,
    pub updated_at: DateTime<Utc>,
    pub issue_iid: Option<i32>,
    pub snapshot: Option<Value>,
}

const ITEM_COLS: &str =
    "id, title, description, acceptance_criteria, status, priority, item_type, labels, \
     assignee_id, is_deleted, updated_at, gitlab_issue_iid, gitlab_snapshot";

fn item_of(r: &sqlx::postgres::PgRow) -> Item {
    Item {
        id: r.get("id"),
        title: r.get("title"),
        description: r.get("description"),
        criteria: r.get("acceptance_criteria"),
        status: r.get("status"),
        priority: r.get("priority"),
        item_type: r.get("item_type"),
        labels: r.get::<Option<Value>, _>("labels").unwrap_or(Value::Null),
        assignee_id: r.get("assignee_id"),
        is_deleted: r.get("is_deleted"),
        updated_at: r.get("updated_at"),
        issue_iid: r.get("gitlab_issue_iid"),
        snapshot: r.get("gitlab_snapshot"),
    }
}

async fn load_item(db: &mut PgConnection, id: Uuid) -> ApiResult<Item> {
    let r = sqlx::query(&format!(
        "SELECT {ITEM_COLS} FROM rnd_backlog_items WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&mut *db)
    .await?;
    Ok(item_of(&r))
}

/// The item's fields as the ERP has them (assignee: the ERP user id).
pub fn erp_side(i: &Item) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("title".into(), json!(i.title.trim()));
    m.insert(
        "description".into(),
        json!(compose(i.description.as_deref(), i.criteria.as_deref())),
    );
    m.insert(
        "status".into(),
        json!(i.status.clone().unwrap_or_else(|| "backlog".into())),
    );
    m.insert("priority".into(), json!(i.priority));
    m.insert("item_type".into(), json!(i.item_type));
    m.insert("labels".into(), json!(plain_labels(&i.labels)));
    m.insert(
        "assignee".into(),
        json!(i.assignee_id.map(|u| u.to_string())),
    );
    m
}

fn scoped<'a>(labels: &'a [String], scope: &str, allowed: &[&str]) -> Option<&'a str> {
    labels
        .iter()
        .filter_map(|l| l.strip_prefix(&format!("{scope}::")))
        .find(|v| allowed.contains(v))
}

/// The issue's fields in the same shape (assignee: the GitLab user id). A scoped label that is missing keeps the
/// last synced value (someone removing priority::high in GitLab does not empty the ERP's priority).
pub fn gl_side(issue: &Value, base: Option<&Map<String, Value>>) -> Map<String, Value> {
    let labels: Vec<String> = issue["labels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let base_of = |k: &str| base.and_then(|b| b.get(k)).cloned().unwrap_or(Value::Null);
    let base_status = base_of("status").as_str().map(str::to_string);
    let status = if issue["state"].as_str() == Some("closed") {
        match base_status.as_deref() {
            Some(s) if is_closed(s) => s.to_string(),
            _ => "done".into(),
        }
    } else if let Some(s) = scoped(&labels, "status", OPEN_STATUSES) {
        s.to_string()
    } else {
        match base_status.as_deref() {
            Some(s) if !is_closed(s) => s.to_string(),
            Some(_) => "in_progress".into(),
            None => "backlog".into(),
        }
    };
    let mut m = Map::new();
    m.insert(
        "title".into(),
        json!(issue["title"].as_str().unwrap_or("").trim()),
    );
    m.insert(
        "description".into(),
        json!(issue["description"].as_str().unwrap_or("").trim()),
    );
    m.insert("status".into(), json!(status));
    m.insert(
        "priority".into(),
        scoped(&labels, "priority", PRIORITIES)
            .map(|v| json!(v))
            .unwrap_or_else(|| base_of("priority")),
    );
    m.insert(
        "item_type".into(),
        scoped(&labels, "type", TYPES)
            .map(|v| json!(v))
            .unwrap_or_else(|| base_of("item_type")),
    );
    m.insert("labels".into(), json!(plain_labels(&issue["labels"])));
    let assignee = issue["assignees"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|u| u["id"].as_i64())
        .or_else(|| issue["assignee"]["id"].as_i64());
    m.insert("assignee".into(), json!(assignee));
    m
}

/// The labels an issue carries for these fields (GitLab representation).
pub fn labels_for(f: &Map<String, Value>) -> Vec<String> {
    let mut out: Vec<String> = f["labels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(s) = f["status"].as_str().filter(|s| !is_closed(s)) {
        out.push(format!("status::{s}"));
    }
    if let Some(p) = f["priority"].as_str() {
        out.push(format!("priority::{p}"));
    }
    if let Some(t) = f["item_type"].as_str() {
        out.push(format!("type::{t}"));
    }
    out
}

// ─── people ──────────────────────────────────────────────────────────────────

/// The GitLab user of an ERP user (link, else by e-mail; a miss is remembered for a day).
pub async fn gl_user_for(db: &mut PgConnection, repo: &Repo, user: Uuid) -> ApiResult<Option<i64>> {
    let link: Option<(Option<i64>, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT gitlab_user_id, matched_by, created_at FROM gitlab_user_links \
         WHERE gitlab_config_id = $1 AND user_id = $2",
    )
    .bind(repo.config_id)
    .bind(user)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((gl, by, at)) = &link {
        if gl.is_some() || (by == "none" && *at > Utc::now() - TimeDelta::days(1)) {
            return Ok(*gl);
        }
    }
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(user)
        .fetch_optional(&mut *db)
        .await?
        .flatten();
    let mut found: Option<(i64, String)> = None;
    if let Some(email) = email.filter(|e| e.contains('@')) {
        if let Ok(Value::Array(users)) =
            ok(repo, Method::GET, "/users", &[("search", &email)], None).await
        {
            let exact = users.iter().find(|u| {
                [u["email"].as_str(), u["public_email"].as_str()]
                    .iter()
                    .flatten()
                    .any(|e| e.eq_ignore_ascii_case(&email))
            });
            let pick = exact.or(if users.len() == 1 {
                users.first()
            } else {
                None
            });
            found = pick.and_then(|u| {
                Some((
                    u["id"].as_i64()?,
                    u["username"].as_str().unwrap_or("").to_string(),
                ))
            });
        }
    }
    sqlx::query("DELETE FROM gitlab_user_links WHERE gitlab_config_id = $1 AND user_id = $2 AND matched_by = 'none'")
        .bind(repo.config_id)
        .bind(user)
        .execute(&mut *db)
        .await?;
    sqlx::query(
        "INSERT INTO gitlab_user_links (id, gitlab_config_id, user_id, gitlab_user_id, gitlab_username, matched_by) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(repo.config_id)
    .bind(user)
    .bind(found.as_ref().map(|f| f.0))
    .bind(found.as_ref().map(|f| f.1.clone()))
    .bind(if found.is_some() { "email" } else { "none" })
    .execute(&mut *db)
    .await?;
    Ok(found.map(|f| f.0))
}

/// The ERP user of a GitLab user (link, else by the GitLab user's e-mail; a miss is remembered for a day).
pub async fn erp_user_for(db: &mut PgConnection, repo: &Repo, gl: i64) -> ApiResult<Option<Uuid>> {
    let link: Option<(Option<Uuid>, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT user_id, matched_by, created_at FROM gitlab_user_links \
         WHERE gitlab_config_id = $1 AND gitlab_user_id = $2",
    )
    .bind(repo.config_id)
    .bind(gl)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((u, by, at)) = &link {
        if u.is_some() || (by == "none" && *at > Utc::now() - TimeDelta::days(1)) {
            return Ok(*u);
        }
    }
    let mut found: Option<Uuid> = None;
    let mut username: Option<String> = None;
    if let Ok(u) = ok(repo, Method::GET, &format!("/users/{gl}"), &[], None).await {
        username = u["username"].as_str().map(str::to_string);
        for e in [u["email"].as_str(), u["public_email"].as_str()]
            .into_iter()
            .flatten()
        {
            if e.contains('@') {
                found = sqlx::query_scalar(
                    "SELECT id FROM users WHERE lower(email) = lower($1) LIMIT 1",
                )
                .bind(e)
                .fetch_optional(&mut *db)
                .await?;
                if found.is_some() {
                    break;
                }
            }
        }
    }
    sqlx::query(
        "DELETE FROM gitlab_user_links WHERE gitlab_config_id = $1 AND gitlab_user_id = $2 AND matched_by = 'none'",
    )
    .bind(repo.config_id)
    .bind(gl)
    .execute(&mut *db)
    .await?;
    if let Some(u) = found {
        // an ERP user already linked to another GitLab account keeps that link
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM gitlab_user_links WHERE gitlab_config_id = $1 AND user_id = $2)",
        )
        .bind(repo.config_id)
        .bind(u)
        .fetch_one(&mut *db)
        .await?;
        if taken {
            return Ok(Some(u));
        }
    }
    sqlx::query(
        "INSERT INTO gitlab_user_links (id, gitlab_config_id, user_id, gitlab_user_id, gitlab_username, matched_by) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(repo.config_id)
    .bind(found)
    .bind(gl)
    .bind(username)
    .bind(if found.is_some() { "email" } else { "none" })
    .execute(&mut *db)
    .await?;
    Ok(found)
}

/// A value in the GitLab representation (the assignee as a GitLab user id).
async fn to_gl(db: &mut PgConnection, repo: &Repo, field: &str, v: &Value) -> ApiResult<Value> {
    if field != "assignee" {
        return Ok(v.clone());
    }
    match v.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(u) => Ok(json!(gl_user_for(db, repo, u).await?)),
        None => Ok(Value::Null),
    }
}

/// A value in the ERP representation; None when it can't be (a GitLab user with no ERP user).
async fn to_erp(
    db: &mut PgConnection,
    repo: &Repo,
    field: &str,
    v: &Value,
) -> ApiResult<Option<Value>> {
    if field != "assignee" {
        return Ok(Some(v.clone()));
    }
    match v.as_i64() {
        Some(gl) => Ok(erp_user_for(db, repo, gl)
            .await?
            .map(|u| json!(u.to_string()))),
        None => Ok(Some(Value::Null)),
    }
}

// ─── history ─────────────────────────────────────────────────────────────────

fn shown(v: &Value) -> PyValue {
    match v {
        Value::Null => PyValue::None,
        Value::String(s) => PyValue::Str(s.chars().take(480).collect()),
        other => PyValue::Str(other.to_string().chars().take(480).collect()),
    }
}

async fn history(
    db: &mut PgConnection,
    item: Uuid,
    verb: &str,
    field: Option<&str>,
    old: &Value,
    new: &Value,
) -> ApiResult<()> {
    crate::collab::record_activity(
        db,
        ENTITY,
        &item.to_string(),
        None,
        verb,
        field,
        Some(&shown(old)),
        Some(&shown(new)),
    )
    .await?;
    Ok(())
}

// ─── one item ────────────────────────────────────────────────────────────────

fn time_of(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

/// Writes the ERP side's pulled fields onto the item (updated_at and gitlab_synced_at the same now()).
async fn apply_pull(
    db: &mut PgConnection,
    item: &Item,
    pull: &Map<String, Value>,
) -> ApiResult<()> {
    let mut title = item.title.clone();
    let mut description = item.description.clone();
    let mut criteria = item.criteria.clone();
    let mut status = item.status.clone();
    let mut priority = item.priority.clone();
    let mut item_type = item.item_type.clone();
    let mut labels = item.labels.clone();
    let mut assignee = item.assignee_id;
    for (k, v) in pull {
        match k.as_str() {
            "title" => title = v.as_str().unwrap_or("").chars().take(500).collect(),
            "description" => {
                let (d, c) = split(v.as_str().unwrap_or(""));
                description = d;
                criteria = c;
            }
            "status" => status = v.as_str().map(str::to_string),
            "priority" => priority = v.as_str().map(str::to_string),
            "item_type" => item_type = v.as_str().map(str::to_string),
            "labels" => labels = v.clone(),
            "assignee" => assignee = v.as_str().and_then(|s| Uuid::parse_str(s).ok()),
            _ => {}
        }
    }
    if title.trim().is_empty() {
        title = item.title.clone();
    }
    sqlx::query(
        "UPDATE rnd_backlog_items SET title = $2, description = $3, acceptance_criteria = $4, status = $5, \
         priority = $6, item_type = $7, labels = $8, assignee_id = $9, \
         resolved_at = CASE WHEN $5 IN ('done', 'released') THEN coalesce(resolved_at, now()) ELSE resolved_at END, \
         updated_at = now() WHERE id = $1",
    )
    .bind(item.id)
    .bind(&title)
    .bind(&description)
    .bind(&criteria)
    .bind(&status)
    .bind(&priority)
    .bind(&item_type)
    .bind(&labels)
    .bind(assignee)
    .execute(&mut *db)
    .await?;
    for (k, v) in pull {
        let old = erp_side(item)
            .get(k.as_str())
            .cloned()
            .unwrap_or(Value::Null);
        history(db, item.id, "gitlab", Some(k), &old, v).await?;
    }
    Ok(())
}

async fn save_synced(
    db: &mut PgConnection,
    item: Uuid,
    erp: &Map<String, Value>,
    gl: &Map<String, Value>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE rnd_backlog_items SET gitlab_snapshot = $2, gitlab_synced_at = greatest(now(), updated_at), \
         gitlab_sync_error = NULL, gitlab_sync_attempts = 0, gitlab_retry_at = NULL WHERE id = $1",
    )
    .bind(item)
    .bind(json!({"erp": erp, "gl": gl}))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Merges an item with its issue (see the module doc). Err: why GitLab refused.
pub async fn reconcile(
    db: &mut PgConnection,
    repo: &Repo,
    item: &Item,
    issue: &Value,
) -> ApiResult<Result<(), String>> {
    let base_erp = item
        .snapshot
        .as_ref()
        .and_then(|s| s.get("erp"))
        .and_then(Value::as_object)
        .cloned();
    let base_gl = item
        .snapshot
        .as_ref()
        .and_then(|s| s.get("gl"))
        .and_then(Value::as_object)
        .cloned();
    let erp = erp_side(item);
    let gl = gl_side(issue, base_gl.as_ref());
    let erp_newer = time_of(&issue["updated_at"]).is_none_or(|t| item.updated_at >= t);
    let mut push = Map::new();
    let mut pull = Map::new();
    for f in FIELDS {
        let ev = erp.get(*f).cloned().unwrap_or(Value::Null);
        let gv = gl.get(*f).cloned().unwrap_or(Value::Null);
        let ec = base_erp.as_ref().is_none_or(|b| b.get(*f) != Some(&ev));
        let gc = base_gl.as_ref().is_none_or(|b| b.get(*f) != Some(&gv));
        if !ec && !gc {
            continue;
        }
        let ev_gl = to_gl(db, repo, f, &ev).await?;
        if ev_gl == gv {
            continue;
        }
        let take_erp = match (ec, gc) {
            (true, false) => true,
            (false, true) => false,
            _ => erp_newer,
        };
        if take_erp {
            push.insert(f.to_string(), ev_gl);
            if gc && base_gl.is_some() {
                // GitLab's edit lost to the newer ERP edit: kept in the item's history
                history(db, item.id, "gitlab_overwritten", Some(f), &gv, &ev).await?;
            }
        } else {
            match to_erp(db, repo, f, &gv).await? {
                Some(v) => {
                    if ec && base_erp.is_some() {
                        history(db, item.id, "erp_overwritten", Some(f), &ev, &v).await?;
                    }
                    pull.insert(f.to_string(), v);
                }
                None => {
                    // assigned in GitLab to someone with no ERP user: noted, the ERP keeps its assignee
                    let who = issue["assignees"][0]["username"].as_str().unwrap_or("?");
                    history(
                        db,
                        item.id,
                        "gitlab",
                        Some("assignee"),
                        &Value::Null,
                        &json!(format!("@{who} in GitLab (no matching ERP user)")),
                    )
                    .await?;
                }
            }
        }
    }
    let iid = issue["iid"].as_i64().unwrap_or(0);
    let mut gl_after = gl.clone();
    if !push.is_empty() {
        let mut fin = gl.clone();
        for (k, v) in &push {
            fin.insert(k.clone(), v.clone());
        }
        let mut body = Map::new();
        if let Some(t) = push.get("title") {
            body.insert("title".into(), t.clone());
        }
        if let Some(d) = push.get("description") {
            body.insert("description".into(), d.clone());
        }
        if ["status", "priority", "item_type", "labels"]
            .iter()
            .any(|k| push.contains_key(*k))
        {
            body.insert("labels".into(), json!(labels_for(&fin).join(",")));
        }
        if push.contains_key("status") {
            let now_closed = fin["status"].as_str().is_some_and(is_closed);
            let was_closed = issue["state"].as_str() == Some("closed");
            if now_closed && !was_closed {
                body.insert("state_event".into(), json!("close"));
            } else if !now_closed && was_closed {
                body.insert("state_event".into(), json!("reopen"));
            }
        }
        if let Some(a) = push.get("assignee") {
            body.insert(
                "assignee_ids".into(),
                match a.as_i64() {
                    Some(id) => json!([id]),
                    None => json!([]),
                },
            );
        }
        match ok(
            repo,
            Method::PUT,
            &issue_path(repo, iid),
            &[],
            Some(&Value::Object(body)),
        )
        .await
        {
            Ok(updated) => gl_after = gl_side(&updated, Some(&fin)),
            Err(why) => return Ok(Err(why)),
        }
    }
    if !pull.is_empty() {
        apply_pull(db, item, &pull).await?;
    }
    let after = load_item(db, item.id).await?;
    save_synced(db, item.id, &erp_side(&after), &gl_after).await?;
    Ok(Ok(()))
}

/// A new issue for an item that has none (a done item's issue is closed straight away).
pub async fn create_issue(
    db: &mut PgConnection,
    repo: &Repo,
    item: &Item,
) -> ApiResult<Result<(), String>> {
    let erp = erp_side(item);
    let assignee = to_gl(db, repo, "assignee", &erp["assignee"]).await?;
    let mut fields = erp.clone();
    fields.insert("assignee".into(), assignee.clone());
    let mut body = json!({
        "title": erp["title"], "description": erp["description"], "labels": labels_for(&fields).join(","),
    });
    if let Some(a) = assignee.as_i64() {
        body["assignee_ids"] = json!([a]);
    }
    let path = format!("/projects/{}/issues", repo.project_id);
    let mut issue = match ok(repo, Method::POST, &path, &[], Some(&body)).await {
        Ok(i) => i,
        Err(why) => return Ok(Err(why)),
    };
    let iid = issue["iid"].as_i64().unwrap_or(0);
    // linked at once: a failure after this never makes a second issue
    sqlx::query(
        "UPDATE rnd_backlog_items SET gitlab_repository_id = $2, gitlab_issue_id = $3, gitlab_issue_iid = $4, \
         gitlab_issue_url = $5 WHERE id = $1",
    )
    .bind(item.id)
    .bind(repo.id)
    .bind(issue["id"].as_i64().and_then(|v| i32::try_from(v).ok()))
    .bind(i32::try_from(iid).ok())
    .bind(issue["web_url"].as_str())
    .execute(&mut *db)
    .await?;
    if erp["status"].as_str().is_some_and(is_closed) {
        match ok(
            repo,
            Method::PUT,
            &issue_path(repo, iid),
            &[],
            Some(&json!({"state_event": "close"})),
        )
        .await
        {
            Ok(i) => issue = i,
            Err(why) => return Ok(Err(why)),
        }
    }
    let gl = gl_side(&issue, Some(&fields));
    save_synced(db, item.id, &erp, &gl).await?;
    Ok(Ok(()))
}

/// A new backlog item for an issue (import, or an issue opened in GitLab).
pub async fn create_item(db: &mut PgConnection, repo: &Repo, issue: &Value) -> ApiResult<Uuid> {
    let gl = gl_side(issue, None);
    let (description, criteria) = split(gl["description"].as_str().unwrap_or(""));
    let assignee = match gl["assignee"].as_i64() {
        Some(g) => erp_user_for(db, repo, g).await?,
        None => None,
    };
    let reporter = match issue["author"]["id"].as_i64() {
        Some(g) => erp_user_for(db, repo, g).await?,
        None => None,
    };
    let status = gl["status"].as_str().unwrap_or("backlog").to_string();
    let id = Uuid::new_v4();
    let title: String = gl["title"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(500)
        .collect();
    sqlx::query(
        "INSERT INTO rnd_backlog_items (id, rnd_project_id, title, description, acceptance_criteria, item_type, \
         status, priority, labels, assignee_id, reporter_id, board_rank, is_deleted, gitlab_repository_id, \
         gitlab_issue_id, gitlab_issue_iid, gitlab_issue_url, resolved_at, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 0, false, $12, $13, $14, $15, \
         CASE WHEN $7 IN ('done', 'released') THEN now() END, coalesce($16, now()), now())",
    )
    .bind(id)
    .bind(repo.rnd_project_id)
    .bind(if title.trim().is_empty() { "(untitled GitLab issue)".to_string() } else { title })
    .bind(&description)
    .bind(&criteria)
    .bind(gl["item_type"].as_str().unwrap_or("feature"))
    .bind(&status)
    .bind(gl["priority"].as_str().unwrap_or("medium"))
    .bind(&gl["labels"])
    .bind(assignee)
    .bind(reporter)
    .bind(repo.id)
    .bind(issue["id"].as_i64().and_then(|v| i32::try_from(v).ok()))
    .bind(issue["iid"].as_i64().and_then(|v| i32::try_from(v).ok()))
    .bind(issue["web_url"].as_str())
    .bind(time_of(&issue["created_at"]))
    .execute(&mut *db)
    .await?;
    history(
        db,
        id,
        "gitlab",
        None,
        &Value::Null,
        &json!(format!("Created from GitLab issue #{}", issue["iid"])),
    )
    .await?;
    let item = load_item(db, id).await?;
    let gl_base = gl_side(issue, Some(&erp_side(&item)));
    save_synced(db, id, &erp_side(&item), &gl_base).await?;
    Ok(id)
}

/// An item deleted in the ERP: its issue is closed and labelled erp::deleted (never deleted), once.
async fn close_deleted(
    db: &mut PgConnection,
    repo: &Repo,
    item: &Item,
) -> ApiResult<Result<(), String>> {
    if let Some(iid) = item.issue_iid {
        let body = json!({"state_event": "close", "add_labels": DELETED_LABEL});
        match call(
            repo,
            Method::PUT,
            &issue_path(repo, iid as i64),
            &[],
            Some(&body),
        )
        .await
        {
            Ok(a) if (200..300).contains(&a.status) || a.status == 404 => {}
            Ok(a) => return Ok(Err(refusal(a.status, &a.body))),
            Err(why) => return Ok(Err(why)),
        }
    }
    sqlx::query(
        "UPDATE rnd_backlog_items SET gitlab_snapshot = jsonb_build_object('deleted', true), \
         gitlab_synced_at = greatest(now(), updated_at), gitlab_sync_error = NULL WHERE id = $1",
    )
    .bind(item.id)
    .execute(&mut *db)
    .await?;
    Ok(Ok(()))
}

/// The issue was deleted in GitLab: the item stays, unlinked, with a history line.
async fn unlink(db: &mut PgConnection, item: &Item) -> ApiResult<()> {
    sqlx::query(
        "UPDATE rnd_backlog_items SET gitlab_repository_id = NULL, gitlab_issue_id = NULL, gitlab_issue_iid = NULL, \
         gitlab_issue_url = NULL, gitlab_snapshot = jsonb_build_object('unlinked', true), \
         gitlab_synced_at = greatest(now(), updated_at), gitlab_sync_error = NULL WHERE id = $1",
    )
    .bind(item.id)
    .execute(&mut *db)
    .await?;
    history(
        db,
        item.id,
        "gitlab",
        None,
        &Value::Null,
        &json!(format!(
            "GitLab issue #{} was deleted; this item is no longer linked to GitLab",
            item.issue_iid.unwrap_or(0)
        )),
    )
    .await
}

async fn item_failed(db: &mut PgConnection, item: Uuid, why: &str) -> ApiResult<()> {
    let max = require_int(db, RETRY_KEY, WHERE).await?.max(1);
    sqlx::query(
        "UPDATE rnd_backlog_items SET gitlab_sync_error = $2, gitlab_sync_attempts = gitlab_sync_attempts + 1, \
         gitlab_retry_at = now() + make_interval(mins => least(power(2, least(gitlab_sync_attempts, 10))::int, $3::int)) \
         WHERE id = $1",
    )
    .bind(item)
    .bind(why.chars().take(1000).collect::<String>())
    .bind(max as i32)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// One issue as GitLab has it: linked item merged, or a new item (an issue closed as erp::deleted is skipped).
async fn sync_issue(
    db: &mut PgConnection,
    repo: &Repo,
    issue: &Value,
) -> ApiResult<Result<(), String>> {
    let iid = issue["iid"].as_i64().unwrap_or(0);
    let row = sqlx::query(&format!(
        "SELECT {ITEM_COLS} FROM rnd_backlog_items WHERE gitlab_repository_id = $1 AND gitlab_issue_iid = $2"
    ))
    .bind(repo.id)
    .bind(iid as i32)
    .fetch_optional(&mut *db)
    .await?;
    match row.map(|r| item_of(&r)) {
        Some(item) if item.is_deleted => Ok(Ok(())),
        Some(item) => reconcile(db, repo, &item, issue).await,
        None => {
            let deleted = issue["labels"]
                .as_array()
                .is_some_and(|a| a.iter().any(|l| l.as_str() == Some(DELETED_LABEL)));
            if !deleted {
                create_item(db, repo, issue).await?;
            }
            Ok(Ok(()))
        }
    }
}

async fn sync_iid(db: &mut PgConnection, repo: &Repo, iid: i64) -> ApiResult<Result<(), String>> {
    match call(repo, Method::GET, &issue_path(repo, iid), &[], None).await {
        Ok(a) if a.status == 404 => {
            let row = sqlx::query(&format!(
                "SELECT {ITEM_COLS} FROM rnd_backlog_items WHERE gitlab_repository_id = $1 AND gitlab_issue_iid = $2"
            ))
            .bind(repo.id)
            .bind(iid as i32)
            .fetch_optional(&mut *db)
            .await?;
            if let Some(r) = row {
                unlink(db, &item_of(&r)).await?;
            }
            Ok(Ok(()))
        }
        Ok(a) if (200..300).contains(&a.status) => sync_issue(db, repo, &a.body).await,
        Ok(a) => Ok(Err(refusal(a.status, &a.body))),
        Err(why) => Ok(Err(why)),
    }
}

// ─── the repository steps ────────────────────────────────────────────────────

async fn ensure_labels(db: &mut PgConnection, repo: &Repo) -> ApiResult<Result<(), String>> {
    if repo.labels_at.is_some() {
        return Ok(Ok(()));
    }
    let mut want: Vec<(String, &str)> = vec![];
    for s in OPEN_STATUSES {
        want.push((format!("status::{s}"), "#428BCA"));
    }
    for p in PRIORITIES {
        let color = match *p {
            "critical" => "#D9534F",
            "high" => "#F0AD4E",
            _ => "#8E8E8E",
        };
        want.push((format!("priority::{p}"), color));
    }
    for t in TYPES {
        want.push((format!("type::{t}"), "#6F42C1"));
    }
    want.push((DELETED_LABEL.to_string(), "#5A5A5A"));
    let path = format!("/projects/{}/labels", repo.project_id);
    for (name, color) in want {
        let body = json!({"name": name, "color": color});
        match call(repo, Method::POST, &path, &[], Some(&body)).await {
            Ok(a) if (200..300).contains(&a.status) || a.status == 409 => {}
            Ok(a) => return Ok(Err(refusal(a.status, &a.body))),
            Err(why) => return Ok(Err(why)),
        }
    }
    sqlx::query("UPDATE gitlab_repositories SET issue_sync_labels_at = now() WHERE id = $1")
        .bind(repo.id)
        .execute(&mut *db)
        .await?;
    Ok(Ok(()))
}

/// The first sync: both ways. Returns (items made from issues, issues made from items).
async fn import(db: &mut PgConnection, repo: &Repo) -> ApiResult<Result<(usize, usize), String>> {
    let issues = match all_pages(
        repo,
        &format!("/projects/{}/issues", repo.project_id),
        &[
            ("state", "all"),
            ("order_by", "created_at"),
            ("sort", "asc"),
        ],
        50,
    )
    .await
    {
        Ok(i) => i,
        Err(why) => return Ok(Err(why)),
    };
    let mut made = 0usize;
    for issue in &issues {
        let iid = issue["iid"].as_i64().unwrap_or(0);
        let linked: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM rnd_backlog_items WHERE gitlab_repository_id = $1 AND gitlab_issue_iid = $2)",
        )
        .bind(repo.id)
        .bind(iid as i32)
        .fetch_one(&mut *db)
        .await?;
        if !linked {
            create_item(db, repo, issue).await?;
            made += 1;
            crate::nary::commit(db).await?;
        }
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rnd_backlog_items WHERE rnd_project_id = $1 AND is_deleted = false \
         AND gitlab_repository_id IS NULL AND gitlab_snapshot IS NULL ORDER BY created_at",
    )
    .bind(repo.rnd_project_id)
    .fetch_all(&mut *db)
    .await?;
    let mut created = 0usize;
    for id in ids {
        let item = load_item(db, id).await?;
        match create_issue(db, repo, &item).await? {
            Ok(()) => created += 1,
            Err(why) => item_failed(db, id, &why).await?,
        }
        crate::nary::commit(db).await?;
    }
    sqlx::query(
        "UPDATE gitlab_repositories SET issue_sync_imported_at = now(), issue_sync_pulled_at = now() WHERE id = $1",
    )
    .bind(repo.id)
    .execute(&mut *db)
    .await?;
    Ok(Ok((made, created)))
}

async fn tell_owner_imported(
    db: &mut PgConnection,
    repo: &Repo,
    made: usize,
    created: usize,
) -> ApiResult<()> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT coalesce(r.name, r.full_path) FROM gitlab_repositories r WHERE r.id = $1",
    )
    .bind(repo.id)
    .fetch_optional(&mut *db)
    .await?
    .flatten();
    let unmatched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM gitlab_user_links WHERE gitlab_config_id = $1 AND matched_by = 'none'",
    )
    .bind(repo.config_id)
    .fetch_one(&mut *db)
    .await?;
    let title = format!("GitLab issue sync started: {}", name.unwrap_or_default());
    let mut body = format!(
        "{made} GitLab issue(s) became backlog items and {created} backlog item(s) got a GitLab issue."
    );
    if unmatched > 0 {
        body.push_str(&format!(
            " {unmatched} person(s) could not be matched between the ERP and GitLab: set them in Settings → GitLab Integration → People."
        ));
    }
    let owner = crate::approvals::routing::owner_user_ids(db).await?;
    let rid = repo.id.to_string();
    crate::approvals::notify::notify(
        db,
        owner,
        crate::approvals::notify::Notice::new(
            &title,
            &body,
            "/settings/gitlab",
            "gitlab_repository",
            &rid,
        )
        .type_("gitlab_sync"),
    )
    .await
}

/// One repository's sync (the module doc's steps). Returns counts for the log.
pub async fn sync_repo(db: &mut PgConnection, repo: &Repo) -> ApiResult<Value> {
    let batch = require_int(db, BATCH_KEY, WHERE).await?.clamp(1, 500);
    let pull_minutes = require_int(db, PULL_KEY, WHERE).await?.max(1);
    if let Err(why) = ensure_labels(db, repo).await? {
        repo_error(db, repo.id, &why).await?;
        return Ok(json!({"repo": repo.id.to_string(), "error": why}));
    }
    if repo.imported_at.is_none() {
        match import(db, repo).await? {
            Ok((made, created)) => {
                tell_owner_imported(db, repo, made, created).await?;
                crate::nary::commit(db).await?;
                return Ok(
                    json!({"repo": repo.id.to_string(), "imported": made, "created": created}),
                );
            }
            Err(why) => {
                repo_error(db, repo.id, &why).await?;
                return Ok(json!({"repo": repo.id.to_string(), "error": why}));
            }
        }
    }
    let mut errors: Vec<String> = vec![];
    // the issues GitLab told about
    let queued: Vec<i32> = sqlx::query_scalar(
        "SELECT issue_iid FROM gitlab_issue_queue WHERE repository_id = $1 ORDER BY queued_at LIMIT $2",
    )
    .bind(repo.id)
    .bind(batch)
    .fetch_all(&mut *db)
    .await?;
    for iid in &queued {
        if let Err(why) = sync_iid(db, repo, *iid as i64).await? {
            errors.push(format!("#{iid}: {why}"));
        }
        sqlx::query("DELETE FROM gitlab_issue_queue WHERE repository_id = $1 AND issue_iid = $2")
            .bind(repo.id)
            .bind(iid)
            .execute(&mut *db)
            .await?;
        crate::nary::commit(db).await?;
    }
    // the pull (a missed webhook is caught)
    let due = repo
        .pulled_at
        .is_none_or(|t| t + TimeDelta::minutes(pull_minutes) <= Utc::now());
    let mut pulled = 0usize;
    if due {
        let since =
            repo.pulled_at.or(repo.imported_at).unwrap_or_else(Utc::now) - TimeDelta::minutes(2);
        let since = since.to_rfc3339();
        match all_pages(
            repo,
            &format!("/projects/{}/issues", repo.project_id),
            &[
                ("state", "all"),
                ("updated_after", &since),
                ("order_by", "updated_at"),
                ("sort", "asc"),
            ],
            10,
        )
        .await
        {
            Ok(issues) => {
                for issue in &issues {
                    if let Err(why) = sync_issue(db, repo, issue).await? {
                        errors.push(format!("#{}: {why}", issue["iid"]));
                    }
                    pulled += 1;
                    crate::nary::commit(db).await?;
                }
                sqlx::query(
                    "UPDATE gitlab_repositories SET issue_sync_pulled_at = now() WHERE id = $1",
                )
                .bind(repo.id)
                .execute(&mut *db)
                .await?;
            }
            Err(why) => errors.push(why),
        }
    }
    // the items changed in the ERP (a new item goes to the project's first syncing repository)
    let rows = sqlx::query(&format!(
        "SELECT {ITEM_COLS} FROM rnd_backlog_items i WHERE i.rnd_project_id = $1 \
         AND (i.gitlab_repository_id = $2 OR (i.gitlab_repository_id IS NULL AND i.gitlab_snapshot IS NULL \
              AND i.is_deleted = false AND $2 = (SELECT r.id FROM gitlab_repositories r WHERE r.rnd_project_id = $1 \
              AND r.issue_sync AND r.is_active ORDER BY r.created_at LIMIT 1))) \
         AND (i.gitlab_synced_at IS NULL OR i.updated_at > i.gitlab_synced_at) \
         AND (i.gitlab_retry_at IS NULL OR i.gitlab_retry_at <= now()) \
         AND coalesce((i.gitlab_snapshot->>'deleted')::boolean, false) = false \
         AND coalesce((i.gitlab_snapshot->>'unlinked')::boolean, false) = false \
         ORDER BY i.updated_at LIMIT $3"
    ))
    .bind(repo.rnd_project_id)
    .bind(repo.id)
    .bind(batch)
    .fetch_all(&mut *db)
    .await?;
    let mut pushed = 0usize;
    for r in rows {
        let item = item_of(&r);
        let res = if item.issue_iid.is_none() {
            if item.is_deleted {
                Ok(())
            } else {
                create_issue(db, repo, &item).await?
            }
        } else if item.is_deleted {
            close_deleted(db, repo, &item).await?
        } else {
            match call(
                repo,
                Method::GET,
                &issue_path(repo, item.issue_iid.unwrap_or(0) as i64),
                &[],
                None,
            )
            .await
            {
                Ok(a) if a.status == 404 => {
                    unlink(db, &item).await?;
                    Ok(())
                }
                Ok(a) if (200..300).contains(&a.status) => {
                    reconcile(db, repo, &item, &a.body).await?
                }
                Ok(a) => Err(refusal(a.status, &a.body)),
                Err(why) => Err(why),
            }
        };
        match res {
            Ok(()) => pushed += 1,
            Err(why) => {
                item_failed(db, item.id, &why).await?;
                errors.push(why);
            }
        }
        crate::nary::commit(db).await?;
    }
    // the ERP comments not yet in GitLab
    let comments = push_comments(db, repo, batch).await?;
    if let Err(why) = &comments {
        errors.push(why.clone());
    }
    sqlx::query(
        "UPDATE gitlab_repositories SET issue_sync_at = now(), issue_sync_error = $2 WHERE id = $1",
    )
    .bind(repo.id)
    .bind(
        errors
            .first()
            .map(|e| e.chars().take(1000).collect::<String>()),
    )
    .execute(&mut *db)
    .await?;
    crate::nary::commit(db).await?;
    Ok(
        json!({"repo": repo.id.to_string(), "queued": queued.len(), "pulled": pulled, "pushed": pushed,
              "comments": comments.unwrap_or(0), "errors": errors.len()}),
    )
}

const MARKER: &str = "<!-- erp-comment:";

fn note_body(name: Option<&str>, body: &str, id: Uuid) -> String {
    format!(
        "**{} (ERP):**\n\n{}\n\n{MARKER}{id} -->",
        name.unwrap_or("Someone"),
        body.trim()
    )
}

fn marker_of(note: &str) -> Option<Uuid> {
    let i = note.find(MARKER)?;
    let rest = &note[i + MARKER.len()..];
    let end = rest.find("-->")?;
    Uuid::parse_str(rest[..end].trim()).ok()
}

async fn push_comments(
    db: &mut PgConnection,
    repo: &Repo,
    batch: i64,
) -> ApiResult<Result<usize, String>> {
    let rows = sqlx::query(
        "SELECT c.id, c.body, u.name, i.gitlab_issue_iid FROM issue_comments c \
         JOIN rnd_backlog_items i ON i.id::text = c.entity_id \
         LEFT JOIN users u ON u.id = c.author_id \
         JOIN gitlab_repositories r ON r.id = i.gitlab_repository_id \
         WHERE c.entity_type = 'rnd_backlog_item' AND c.gitlab_note_id IS NULL \
         AND coalesce(c.origin, 'erp') = 'erp' AND i.gitlab_repository_id = $1 AND i.gitlab_issue_iid IS NOT NULL \
         AND c.created_at > r.issue_sync_imported_at AND i.is_deleted = false \
         ORDER BY c.created_at LIMIT $2",
    )
    .bind(repo.id)
    .bind(batch)
    .fetch_all(&mut *db)
    .await?;
    let mut sent = 0usize;
    for r in rows {
        let id: Uuid = r.get("id");
        let body: String = r.get("body");
        let name: Option<String> = r.get("name");
        let iid: i32 = r.get("gitlab_issue_iid");
        let path = format!("{}/notes", issue_path(repo, iid as i64));
        let note = json!({"body": note_body(name.as_deref(), &body, id)});
        match ok(repo, Method::POST, &path, &[], Some(&note)).await {
            Ok(n) => {
                // the webhook may have linked it first (it carries the marker)
                sqlx::query(
                    "UPDATE issue_comments SET gitlab_note_id = coalesce(gitlab_note_id, $2), origin = 'erp', \
                     gitlab_sync_error = NULL WHERE id = $1",
                )
                .bind(id)
                .bind(n["id"].as_i64())
                .execute(&mut *db)
                .await?;
                sent += 1;
            }
            Err(why) => {
                sqlx::query("UPDATE issue_comments SET gitlab_sync_error = $2 WHERE id = $1")
                    .bind(id)
                    .bind(&why)
                    .execute(&mut *db)
                    .await?;
                crate::nary::commit(db).await?;
                return Ok(Err(why));
            }
        }
        crate::nary::commit(db).await?;
    }
    Ok(Ok(sent))
}

/// Every syncing repository (the worker's tick). One repository's failure never stops the others.
pub async fn tick(db: &mut PgConnection) -> ApiResult<Value> {
    let mut out = vec![];
    for repo in repos(db, None).await? {
        out.push(sync_repo(db, &repo).await?);
    }
    Ok(json!({"repos": out}))
}

// ─── the webhook ─────────────────────────────────────────────────────────────

/// Whether a repository syncs its issues (the webhook's question).
pub async fn syncing(db: &mut PgConnection, repo: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT issue_sync AND issue_sync_imported_at IS NOT NULL FROM gitlab_repositories WHERE id = $1",
    )
    .bind(repo)
    .fetch_optional(&mut *db)
    .await?
    .unwrap_or(false))
}

/// An issue event: the issue is synced by the next tick.
pub async fn issue_event(db: &mut PgConnection, repo: Uuid, payload: &Value) -> ApiResult<()> {
    let Some(iid) = payload["object_attributes"]["iid"].as_i64() else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO gitlab_issue_queue (repository_id, issue_iid) VALUES ($1, $2) \
         ON CONFLICT (repository_id, issue_iid) DO UPDATE SET queued_at = now()",
    )
    .bind(repo)
    .bind(iid as i32)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// A comment event: a new GitLab comment on a synced issue becomes an ERP comment (by the matched ERP user, else
/// with the GitLab username); our own (marked) comments coming back are only linked. Edits and system notes are
/// not synced.
pub async fn note_event(
    db: &mut PgConnection,
    repo: Uuid,
    config: Uuid,
    payload: &Value,
) -> ApiResult<()> {
    let n = &payload["object_attributes"];
    if n["noteable_type"].as_str() != Some("Issue") || n["system"].as_bool() == Some(true) {
        return Ok(());
    }
    if matches!(n["action"].as_str(), Some(a) if a != "create") {
        return Ok(());
    }
    let Some(note_id) = n["id"].as_i64() else {
        return Ok(());
    };
    let body = n["note"].as_str().unwrap_or("");
    if let Some(ours) = marker_of(body) {
        sqlx::query(
            "UPDATE issue_comments SET gitlab_note_id = $2, origin = 'erp' WHERE id = $1 AND gitlab_note_id IS NULL",
        )
        .bind(ours)
        .bind(note_id)
        .execute(&mut *db)
        .await?;
        return Ok(());
    }
    let Some(iid) = payload["issue"]["iid"].as_i64() else {
        return Ok(());
    };
    let item: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rnd_backlog_items WHERE gitlab_repository_id = $1 AND gitlab_issue_iid = $2 AND is_deleted = false",
    )
    .bind(repo)
    .bind(iid as i32)
    .fetch_optional(&mut *db)
    .await?;
    let Some(item) = item else {
        return Ok(());
    };
    let author: Option<Uuid> = match payload["user"]["id"].as_i64() {
        Some(g) => sqlx::query_scalar(
            "SELECT user_id FROM gitlab_user_links WHERE gitlab_config_id = $1 AND gitlab_user_id = $2 \
             AND user_id IS NOT NULL",
        )
        .bind(config)
        .bind(g)
        .fetch_optional(&mut *db)
        .await?,
        None => None,
    };
    let username = payload["user"]["username"].as_str().map(str::to_string);
    let text = match (&author, &username) {
        (None, Some(u)) => format!("@{u} (GitLab): {body}"),
        _ => body.to_string(),
    };
    let inserted = sqlx::query(
        "INSERT INTO issue_comments (id, entity_type, entity_id, author_id, body, mentions, gitlab_note_id, origin, \
         gitlab_author) VALUES ($1, $2, $3, $4, $5, '[]'::jsonb, $6, 'gitlab', $7) ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(ENTITY)
    .bind(item.to_string())
    .bind(author)
    .bind(&text)
    .bind(note_id)
    .bind(&username)
    .execute(&mut *db)
    .await?
    .rows_affected();
    if inserted > 0 {
        crate::collab::record_activity(
            db,
            ENTITY,
            &item.to_string(),
            author,
            "comment",
            None,
            None,
            None,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_and_criteria_round_trip() {
        let c = compose(Some("Build the portal."), Some("- works offline"));
        assert_eq!(
            c,
            "Build the portal.\n\n## Acceptance criteria\n\n- works offline"
        );
        assert_eq!(
            split(&c),
            (
                Some("Build the portal.".into()),
                Some("- works offline".into())
            )
        );
        assert_eq!(split("Just text"), (Some("Just text".into()), None));
        assert_eq!(compose(None, Some("x")), "## Acceptance criteria\n\nx");
        assert_eq!(
            split("## Acceptance criteria\n\nx"),
            (None, Some("x".into()))
        );
    }

    #[test]
    fn labels_carry_status_priority_and_type() {
        let mut f = Map::new();
        f.insert("labels".into(), json!(["ux"]));
        f.insert("status".into(), json!("in_progress"));
        f.insert("priority".into(), json!("high"));
        f.insert("item_type".into(), json!("bug"));
        assert_eq!(
            labels_for(&f),
            vec!["ux", "status::in_progress", "priority::high", "type::bug"]
        );
        f.insert("status".into(), json!("done"));
        assert!(!labels_for(&f).iter().any(|l| l.starts_with("status::")));
        let issue = json!({"title": "T", "description": "", "state": "opened",
                           "labels": ["ux", "status::review", "priority::low", "type::bug", "erp::deleted"]});
        let g = gl_side(&issue, None);
        assert_eq!(g["status"], "review");
        assert_eq!(g["priority"], "low");
        assert_eq!(g["labels"], json!(["ux"]));
        // closed: done; a missing priority label keeps the synced value
        let closed = json!({"title": "T", "state": "closed", "labels": []});
        let mut base = Map::new();
        base.insert("priority".into(), json!("critical"));
        base.insert("status".into(), json!("in_progress"));
        let g = gl_side(&closed, Some(&base));
        assert_eq!(g["status"], "done");
        assert_eq!(g["priority"], "critical");
        // reopened without a status label: back to work
        base.insert("status".into(), json!("done"));
        let g = gl_side(
            &json!({"title": "T", "state": "opened", "labels": []}),
            Some(&base),
        );
        assert_eq!(g["status"], "in_progress");
    }

    #[test]
    fn our_comments_carry_a_marker() {
        let id = Uuid::new_v4();
        let b = note_body(Some("Diana"), "Looks good", id);
        assert!(b.starts_with("**Diana (ERP):**"));
        assert_eq!(marker_of(&b), Some(id));
        assert_eq!(marker_of("plain"), None);
    }
}
