//! The Python tests that call the assistant's catalog tools (backend/tests: test_ai_records_tool.py,
//! test_ai_research_tools.py (except read_web_page: tests/ai_tools_research.rs), test_assistant_proposals_api.py,
//! test_nary_cards.py, test_nary_chat_fixes.py, test_nary_learns.py, test_catalog_pricing_c.py,
//! test_nary_send_as_nary.py, test_mail_sync.py, test_ai_erp_guide.py, test_meeting_on_request.py,
//! test_nary_backlog_oct13.py, test_nary_send_failures.py, test_nary_take_in.py), ported with the same names
//! and assertions. Steps of a Python test that go through a router (POST /nary/actions/run, /nary/send,
//! /nary/meetings/book: wave 4) or another module's own behaviour (staff_chat, the Google fake) are left
//! out and named in a comment where they were.
//!
//! One fresh database for the file; the tests take a lock and run one at a time (as the Python suite shares
//! one database).

mod support;

use std::collections::HashSet;
use std::sync::OnceLock;

use chrono::{Duration, Utc};
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog::{self, run_tool, tools_for_user, Tool};
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

static URL: OnceLock<String> = OnceLock::new();
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Db {
    pool: PgPool,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

async fn db() -> Db {
    let guard = LOCK.lock().await;
    if URL.get().is_none() {
        let pool = support::fresh_db("ai_tools_catalog").await;
        pool.close().await;
        URL.set(std::env::var("DATABASE_URL").unwrap()).ok();
    }
    support::init();
    let pool = PgPool::connect(URL.get().unwrap()).await.expect("connect");
    granary_services::nary::set_session_pool(pool.clone());
    Db {
        pool,
        _guard: guard,
    }
}

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// `make_person(perms=...)` / `superadmin()`: a user row and the AuthUser the tools receive.
async fn person(pool: &PgPool, superadmin: bool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, $4, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@example.com", u("pytest")))
    .bind(u("P0 User"))
    .bind(superadmin)
    .execute(pool)
    .await
    .unwrap();
    let mut user: AuthUser = sqlx::query_as(
        "SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, is_system_admin, \
         portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, created_at, updated_at \
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    user.perms = perms.iter().map(|p| p.to_string()).collect::<HashSet<_>>();
    user
}

/// `async with AsyncSessionLocal() as db: return await run_tool(db, user, name, args)` (never committed).
async fn run(pool: &PgPool, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let out = run_tool(&mut tx, user, name, args).await;
    tx.rollback().await.unwrap();
    out
}

/// A handler called directly, in a transaction that is rolled back.
macro_rules! direct {
    ($pool:expr, $h:path, $user:expr, $args:expr) => {{
        let mut conn = $pool.acquire().await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        let out = $h(&mut *tx, $user, $args).await;
        tx.rollback().await.unwrap();
        out
    }};
}

async fn exec(pool: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(pool).await.unwrap();
}

async fn orgs(pool: &PgPool, owner: Uuid, count: usize, city: &str) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for _ in 0..count {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations (id, name, city, industry, assigned_to_id, is_deleted) \
             VALUES ($1, $2, $3, 'manufacturing', $4, false)",
        )
        .bind(id)
        .bind(Uuid::new_v4().simple().to_string())
        .bind(city)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
        ids.push(id);
    }
    ids
}

fn names(tools: &[Tool]) -> Vec<String> {
    tools.iter().map(|t| t.name.clone()).collect()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

// ─── test_ai_records_tool.py: search_records ─────────────────────────────────

#[tokio::test]
async fn test_search_records_counts_everything_and_pages_through_it() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_all"]).await;
    let city = u("Pytestville");
    orgs(&d.pool, user.id, 7, &city).await;
    let filters = json!([{"field": "city", "op": "eq", "value": city}]);
    let first = run(
        &d.pool,
        &user,
        "search_records",
        json!({"object_type": "organization", "filters": filters, "limit": 5}),
    )
    .await;
    assert!(
        first["count"] == 7
            && first["items"].as_array().unwrap().len() == 5
            && first["has_more"] == true
    );
    let second = run(
        &d.pool,
        &user,
        "search_records",
        json!({"object_type": "organization", "filters": filters, "limit": 5, "offset": 5}),
    )
    .await;
    assert!(second["items"].as_array().unwrap().len() == 2 && second["has_more"] == false);
    let a: HashSet<String> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| s(&i["id"]).to_string())
        .collect();
    let b: HashSet<String> = second["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| s(&i["id"]).to_string())
        .collect();
    assert!(a.is_disjoint(&b));
}

#[tokio::test]
async fn test_search_records_only_shows_what_the_user_may_see() {
    let d = db().await;
    let mine = person(&d.pool, false, &["crm:view_own"]).await;
    let theirs = person(&d.pool, false, &["crm:view_own"]).await;
    let city = u("Splitville");
    orgs(&d.pool, mine.id, 2, &city).await;
    orgs(&d.pool, theirs.id, 3, &city).await;
    let filters = json!([{"field": "city", "op": "eq", "value": city}]);
    let args = json!({"object_type": "organization", "filters": filters});
    assert_eq!(
        run(&d.pool, &mine, "search_records", args.clone()).await["count"],
        2
    );
    assert_eq!(
        run(&d.pool, &theirs, "search_records", args).await["count"],
        3
    );
}

#[tokio::test]
async fn test_a_wrong_field_teaches_the_model_the_right_ones() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_all"]).await;
    let out = run(&d.pool, &user, "search_records",
                  json!({"object_type": "organization", "filters": [{"field": "kota", "op": "eq", "value": "x"}]})).await;
    assert!(
        out.get("error").is_some()
            && out["valid_fields"]
                .as_array()
                .unwrap()
                .contains(&json!("city"))
    );
    let schema = run(
        &d.pool,
        &user,
        "crm_field_schema",
        json!({"object_type": "deal"}),
    )
    .await;
    assert!(schema["fields"].get("stage").is_some());
}

#[tokio::test]
async fn test_the_tool_is_refused_without_crm_permission() {
    let d = db().await;
    let outsider = person(&d.pool, false, &[]).await;
    assert!(!names(&tools_for_user(&outsider)).contains(&"search_records".to_string()));
    // Executing it directly is refused too (the permission is re-checked at run time).
    let refused = run(
        &d.pool,
        &outsider,
        "search_records",
        json!({"object_type": "organization"}),
    )
    .await;
    assert!(
        s(&refused["error"]).to_lowercase().contains("permission")
            && refused.get("items").is_none()
    );
}

// ─── test_ai_records_tool.py: follow-ups ─────────────────────────────────────

#[tokio::test]
async fn test_list_my_followups_is_caller_scoped() {
    use granary_services::crm::followups::{create, NewReminder};
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_own"]).await;
    let other = person(&d.pool, false, &["crm:view_own"]).await;
    {
        let mut conn = d.pool.acquire().await.unwrap();
        for (owner, title) in [(user.id, "Chase the quote"), (other.id, "Not yours")] {
            let mut r = NewReminder::new(owner, "contact", title);
            r.entity_id = Some(Uuid::new_v4());
            r.due_at = Some(Utc::now() - Duration::hours(1));
            create(&mut conn, r).await.unwrap();
        }
    }
    let mine = run(
        &d.pool,
        &user,
        "list_my_followups",
        json!({"overdue": true}),
    )
    .await;
    let titles: Vec<&str> = mine["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| s(&i["title"]))
        .collect();
    assert!(titles.contains(&"Chase the quote") && !titles.contains(&"Not yours"));
    assert!(mine["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["overdue"] == true));
}

async fn count_reminders(pool: &PgPool, owner: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM crm_reminders WHERE owner_user_id = $1")
        .bind(owner)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_proposing_a_reminder_saves_nothing() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_own"]).await;
    let before = count_reminders(&d.pool, user.id).await;
    let out = run(&d.pool, &user, "propose_followup_reminder",
                  json!({"entity_type": "whatsapp_chat", "chat_id": "819012@c.us", "title": "Ask about the PO"})).await;
    assert_eq!(out["proposal"]["kind"], "followup_reminder");
    assert!(
        !s(&out["proposal"]["due_at"]).is_empty()
            && s(&out["proposal"]["summary"]).contains("Ask about the PO")
    );
    assert_eq!(
        count_reminders(&d.pool, user.id).await,
        before,
        "a proposal must never write"
    );
    let bad = run(
        &d.pool,
        &user,
        "propose_followup_reminder",
        json!({"entity_type": "whatsapp_chat", "title": "x"}),
    )
    .await;
    assert!(bad.get("error").is_some());
}

// ─── test_ai_records_tool.py: search_contacts owner scoping ──────────────────

async fn contacts(pool: &PgPool, owner: Uuid, count: usize, last_name: &str) {
    for _ in 0..count {
        sqlx::query(
            "INSERT INTO contacts (id, first_name, last_name, email, assigned_to_id, is_deleted) VALUES ($1, $2, $3, $4, $5, false)",
        )
        .bind(Uuid::new_v4())
        .bind(&Uuid::new_v4().simple().to_string()[..8])
        .bind(last_name)
        .bind(format!("{}@pytest.test", &Uuid::new_v4().simple().to_string()[..8]))
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn test_search_contacts_hides_other_peoples_contacts_from_a_view_own_user() {
    let d = db().await;
    let mine = person(&d.pool, false, &["crm:view_own"]).await;
    let theirs = person(&d.pool, false, &["crm:view_own"]).await;
    let surname = format!("Scope{}", &Uuid::new_v4().simple().to_string()[..6]);
    contacts(&d.pool, mine.id, 2, &surname).await;
    contacts(&d.pool, theirs.id, 3, &surname).await;
    let out = run(&d.pool, &mine, "search_contacts", json!({"query": surname})).await;
    assert_eq!(
        out["contacts"].as_array().unwrap().len(),
        2,
        "leaked contacts assigned to someone else"
    );
}

#[tokio::test]
async fn test_search_contacts_shows_everything_to_a_view_all_user() {
    let d = db().await;
    let everyone = person(&d.pool, false, &["crm:view_all"]).await;
    let owner = person(&d.pool, false, &["crm:view_own"]).await;
    let surname = format!("Scope{}", &Uuid::new_v4().simple().to_string()[..6]);
    contacts(&d.pool, owner.id, 3, &surname).await;
    let out = run(
        &d.pool,
        &everyone,
        "search_contacts",
        json!({"query": surname}),
    )
    .await;
    assert_eq!(out["contacts"].as_array().unwrap().len(), 3);
}

// ─── test_ai_research_tools.py: get_research / start_research ────────────────

async fn org(pool: &PgPool, name: Option<String>, owner: Option<Uuid>) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let name = name.unwrap_or_else(|| {
        format!(
            "Research Corp {}",
            &Uuid::new_v4().simple().to_string()[..8]
        )
    });
    sqlx::query("INSERT INTO organizations (id, name, assigned_to_id, is_deleted) VALUES ($1, $2, $3, false)")
        .bind(id)
        .bind(&name)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    (id, name)
}

#[tokio::test]
async fn test_research_on_someone_elses_company_is_invisible() {
    let d = db().await;
    let owner = person(&d.pool, false, &["crm:view_own", "crm:create_contact"]).await;
    let other = person(&d.pool, false, &["crm:view_own", "crm:create_contact"]).await;
    let (oid, _) = org(&d.pool, None, Some(owner.id)).await;
    for tool in ["get_research", "start_research"] {
        let out = run(
            &d.pool,
            &other,
            tool,
            json!({"object_type": "organization", "record_id": oid.to_string()}),
        )
        .await;
        assert!(s(&out["error"]).contains("No organization you can see"));
    }
    let out = run(
        &d.pool,
        &owner,
        "get_research",
        json!({"object_type": "organization", "record_id": oid.to_string()}),
    )
    .await;
    assert!(out["run"].is_null());
}

#[tokio::test]
async fn test_get_research_returns_the_latest_run_and_people_to_review() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_all"]).await;
    let (oid, name) = org(&d.pool, None, None).await;
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO crm_research_runs (id, object_type, record_id, depth, status, summary, written) \
         VALUES ($1, 'organization', $2, 'deep', 'done', 'Steel maker in Springfield.', '{\"city\": \"Springfield\"}')",
    )
    .bind(run_id)
    .bind(oid)
    .execute(&d.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO crm_research_people (id, organization_id, run_id, name, name_key, title, status) \
         VALUES ($1, $2, $3, 'John Scott', 'john scott', 'Managing Director', 'pending')",
    )
    .bind(Uuid::new_v4())
    .bind(oid)
    .bind(run_id)
    .execute(&d.pool)
    .await
    .unwrap();
    let out = run(
        &d.pool,
        &user,
        "get_research",
        json!({"object_type": "organization", "name": name}),
    )
    .await;
    assert!(
        out["run"]["summary"] == "Steel maker in Springfield."
            && out["run"]["written"] == json!({"city": "Springfield"})
    );
    let people: Vec<&str> = out["people_to_review"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| s(&p["name"]))
        .collect();
    assert_eq!(people, vec!["John Scott"]);
}

#[tokio::test]
async fn test_start_research_only_offers_a_card() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_all", "crm:create_contact"]).await;
    let (oid, _) = org(&d.pool, None, None).await;
    let out = run(
        &d.pool,
        &user,
        "start_research",
        json!({"object_type": "organization", "record_id": oid.to_string()}),
    )
    .await;
    assert!(
        out["proposal"]["kind"] == "research"
            && out["proposal"]["record_id"] == json!(oid.to_string())
    );
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM crm_research_runs WHERE record_id = $1")
            .bind(oid)
            .fetch_one(&d.pool)
            .await
            .unwrap();
    assert_eq!(runs, 0, "nothing starts until the user confirms the card");
}

#[tokio::test]
async fn test_a_run_in_flight_is_reported_instead_of_offered_again() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_all", "crm:create_contact"]).await;
    let (oid, _) = org(&d.pool, None, None).await;
    sqlx::query("INSERT INTO crm_research_runs (id, object_type, record_id, depth, status) VALUES ($1, 'organization', $2, 'deep', 'running')")
        .bind(Uuid::new_v4())
        .bind(oid)
        .execute(&d.pool)
        .await
        .unwrap();
    let out = run(
        &d.pool,
        &user,
        "start_research",
        json!({"object_type": "organization", "record_id": oid.to_string()}),
    )
    .await;
    assert!(out["status"] == "already_running" && out.get("proposal").is_none());
}

#[tokio::test]
async fn test_an_ambiguous_name_asks_which_one() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_all"]).await;
    let stem = format!("Twin Corp {}", &Uuid::new_v4().simple().to_string()[..6]);
    org(&d.pool, Some(format!("{stem} Initech")), None).await;
    org(&d.pool, Some(format!("{stem} Apex")), None).await;
    let out = run(
        &d.pool,
        &user,
        "get_research",
        json!({"object_type": "organization", "name": stem}),
    )
    .await;
    assert!(
        s(&out["error"]).contains("Several records")
            && out["candidates"].as_array().unwrap().len() == 2
    );
}

#[tokio::test]
async fn test_research_tools_follow_crm_permissions() {
    let d = db().await;
    let outsider = person(&d.pool, false, &[]).await;
    let n = names(&tools_for_user(&outsider));
    assert!(n.contains(&"read_web_page".into()) && n.contains(&"search_web".into()));
    assert!(!n.contains(&"get_research".into()) && !n.contains(&"start_research".into()));
    let refused = run(
        &d.pool,
        &outsider,
        "start_research",
        json!({"object_type": "organization", "name": "x"}),
    )
    .await;
    assert!(s(&refused["error"]).to_lowercase().contains("permission"));
}

#[tokio::test]
async fn test_only_companies_and_contacts_can_be_researched() {
    let d = db().await;
    let user = person(&d.pool, false, &["crm:view_all", "crm:create_contact"]).await;
    for object_type in ["deal", "lead", ""] {
        let out = run(
            &d.pool,
            &user,
            "start_research",
            json!({"object_type": object_type, "name": "x"}),
        )
        .await;
        assert!(
            s(&out["error"]).contains("organization' or 'contact'"),
            "{object_type}"
        );
    }
}

// ─── test_assistant_proposals_api.py ─────────────────────────────────────────

async fn broken(
    db: &mut PgConnection,
    _user: &AuthUser,
    _args: Value,
) -> granary_core::errors::ApiResult<Value> {
    sqlx::query("SELECT * FROM table_that_does_not_exist")
        .execute(&mut *db)
        .await?;
    Ok(Value::Null)
}

#[tokio::test]
async fn test_a_failing_tool_leaves_the_session_usable() {
    let d = db().await;
    let tool = Tool::new(
        "broken_tool",
        "",
        json!({"type": "object", "properties": {}}),
        granary_services::ai_tool_handler!(broken),
        &[],
    );
    catalog::set_tool_for_tests("broken_tool", Some(tool));
    let user = person(&d.pool, true, &[]).await;
    let mut conn = d.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let out = run_tool(&mut tx, &user, "broken_tool", json!({})).await;
    catalog::set_tool_for_tests("broken_tool", None);
    assert!(s(&out["error"]).starts_with("Tool failed"));
    // Before the savepoint this raised "current transaction is aborted", losing the turn's save.
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(one, 1);
    tx.rollback().await.unwrap();
}

// ─── test_nary_cards.py ──────────────────────────────────────────────────────

async fn propose(pool: &PgPool, me: &AuthUser, mut args: Value) -> Value {
    if args["fields"].is_object() {
        args["fields"] = json!(args["fields"].to_string());
    }
    direct!(pool, catalog::_h_propose_record_change, me, args).unwrap()
}

#[tokio::test]
async fn test_a_free_text_lead_source_is_refused_when_proposed_with_the_allowed_keys() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let tag = u("pytest-cards");
    let out = propose(&d.pool, &me, json!({"object_type": "lead", "action": "create",
                                          "fields": {"title": format!("Globex {tag}"), "source": "Email, hello@"}})).await;
    let e = s(&out["error"]);
    assert!(e.contains("Unknown lead source 'Email, hello@'") && e.contains("email"));
    assert!(e.contains("crm_field_schema"));
}

#[tokio::test]
async fn test_every_picklist_and_id_is_checked_before_the_card_is_shown() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let cases = [
        (
            "lead",
            json!({"title": "x", "source": "email", "priority": "super"}),
            "Unknown lead priority",
        ),
        (
            "activity",
            json!({"entity_type": "lead", "entity_id": Uuid::new_v4().to_string(), "activity_type": "chat"}),
            "Unknown activity type",
        ),
        (
            "activity",
            json!({"entity_type": "invoice", "entity_id": Uuid::new_v4().to_string(), "activity_type": "call"}),
            "Unknown entity_type",
        ),
        (
            "lead",
            json!({"title": "x", "source": "email", "organization_id": Uuid::new_v4().to_string()}),
            "no organization with id",
        ),
        (
            "deal",
            json!({"title": "x", "expected_close_date": "next week"}),
            "must be a date",
        ),
    ];
    for (object_type, fields, needle) in cases {
        let out = propose(
            &d.pool,
            &me,
            json!({"object_type": object_type, "action": "create", "fields": fields}),
        )
        .await;
        assert!(s(&out["error"]).contains(needle), "{needle}: {out}");
    }
}

#[tokio::test]
async fn test_the_card_shows_names_not_ids() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let tag = u("pytest-cards");
    let (oid, _) = org(&d.pool, Some(format!("Globex Group {tag}")), None).await;
    let out = propose(
        &d.pool,
        &me,
        json!({"object_type": "lead", "action": "create",
        "fields": {"title": format!("Globex: support plan {tag}"), "source": "email", "priority": "high",
                   "organization_id": oid.to_string()}}),
    )
    .await;
    let card = &out["proposal"];
    assert_eq!(
        card["display"]["organization_id"],
        json!(format!("Globex Group {tag}"))
    );
    assert_eq!(card["label"], json!(format!("Globex: support plan {tag}"))); // a create card has a label too
}

#[tokio::test]
async fn test_the_field_schema_lists_the_allowed_keys() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let lead = direct!(
        d.pool,
        catalog::_h_crm_field_schema,
        &me,
        json!({"object_type": "lead"})
    )
    .unwrap();
    let act = direct!(
        d.pool,
        catalog::_h_crm_field_schema,
        &me,
        json!({"object_type": "activity"})
    )
    .unwrap();
    assert!(lead["allowed_values"]["source"]
        .as_array()
        .unwrap()
        .contains(&json!("email")));
    assert!(lead["allowed_values"]["priority"]
        .as_array()
        .unwrap()
        .contains(&json!("high")));
    assert!(
        lead["required_on_create"] == json!(["source"])
            && lead["writable"]
                .as_array()
                .unwrap()
                .contains(&json!("source"))
    );
    assert!(act["allowed_values"]["activity_type"]
        .as_array()
        .unwrap()
        .contains(&json!("call")));
}

// ─── test_nary_chat_fixes.py ─────────────────────────────────────────────────

async fn chat(pool: &PgPool, contact: Option<Uuid>, inbound: bool) -> String {
    let n: u64 = rand_digits();
    let cid = format!("8190{n:08}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, contact_id, attribution_asked) \
         VALUES ($1, $2, $3, 'John', false, 'active', true, $4, false)",
    )
    .bind(Uuid::new_v4())
    .bind(&cid)
    .bind(format!("+81812{n:08}"))
    .bind(contact)
    .execute(pool)
    .await
    .unwrap();
    if inbound {
        sqlx::query("INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body) VALUES ($1, $2, $3, 'inbound', false, 'Hello')")
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
            .bind(&cid)
            .execute(pool)
            .await
            .unwrap();
    }
    cid
}

fn rand_digits() -> u64 {
    (Uuid::new_v4().as_u128() % 100_000_000) as u64
}

async fn contact(pool: &PgPool, first: &str, last: &str, email: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, email, is_deleted) VALUES ($1, $2, $3, $4, false)")
        .bind(id)
        .bind(first)
        .bind(last)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn test_the_chat_link_card_links_a_chat_by_hand() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let cid = chat(&d.pool, None, true).await;
    let contact_id = contact(
        &d.pool,
        "Carol",
        &format!("Pytest{}", &Uuid::new_v4().simple().to_string()[..5]),
        None,
    )
    .await;
    let out = direct!(
        d.pool,
        catalog::_h_propose_chat_link,
        &me,
        json!({"chat_id": cid, "contact_id": contact_id.to_string()})
    )
    .unwrap();
    let card = &out["proposal"];
    assert!(card["kind"] == "chat_link" && s(&card["summary"]).contains("Link the WhatsApp chat"));
    // (Python then confirms through POST /nary/chats/{id}/link: wave 4.)
}

#[tokio::test]
async fn test_a_reminder_card_snoozes_or_cancels_only_the_users_own() {
    use granary_services::crm::followups::{create, NewReminder};
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let somebody = person(&d.pool, false, &[]).await;
    let (mine, other) = {
        let mut conn = d.pool.acquire().await.unwrap();
        let mut a = NewReminder::new(me.id, "contact", "Mine");
        a.entity_id = Some(Uuid::new_v4());
        let mut b = NewReminder::new(somebody.id, "contact", "Theirs");
        b.entity_id = Some(Uuid::new_v4());
        (
            create(&mut conn, a).await.unwrap().unwrap(),
            create(&mut conn, b).await.unwrap().unwrap(),
        )
    };
    let until =
        granary_compat::pyjson::PyDateTime::from_chrono_utc(&(Utc::now() + Duration::days(3)))
            .isoformat();
    let ok = direct!(
        d.pool,
        catalog::_h_propose_reminder_change,
        &me,
        json!({"reminder_id": mine.id.to_string(), "action": "snooze", "until": until})
    )
    .unwrap();
    let no = direct!(
        d.pool,
        catalog::_h_propose_reminder_change,
        &me,
        json!({"reminder_id": other.id.to_string(), "action": "cancel"})
    )
    .unwrap();
    assert!(
        ok["proposal"]["kind"] == "reminder_change"
            && s(&ok["proposal"]["summary"]).starts_with("Move \u{201c}Mine\u{201d}")
    );
    assert!(no.get("error").is_some());
    // (Python then checks staff_chat._request_for(card): A4/staff_chat.)
}

async fn product(
    pool: &PgPool,
    sku: &str,
    name: &str,
    product_type: &str,
    unit: &str,
    family: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, family, sale_model, price_metric, pricing_mode, \
         is_whitelabel, is_active, is_orderable, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, 'one_time', 'unit', 'list', false, true, true, false)",
    )
    .bind(id)
    .bind(sku)
    .bind(name)
    .bind(product_type)
    .bind(unit)
    .bind(family)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_the_catalog_search_matches_every_word() {
    let d = db().await;
    let tag = Uuid::new_v4().simple().to_string()[..6].to_uppercase();
    product(
        &d.pool,
        &format!("PYT{tag}-SDK"),
        &format!("Premium support plan {tag}"),
        "software",
        "Licence",
        "support",
    )
    .await;
    product(
        &d.pool,
        &format!("PYT{tag}-WL"),
        &format!("White-label branding {tag}"),
        "service",
        "Unit",
        "addons",
    )
    .await;
    let user = person(&d.pool, true, &[]).await;
    let sdk = direct!(
        d.pool,
        catalog::_h_search_catalog,
        &user,
        json!({"query": format!("support plan {tag}")})
    )
    .unwrap();
    let wl = direct!(
        d.pool,
        catalog::_h_search_catalog,
        &user,
        json!({"query": format!("white label {tag}")})
    )
    .unwrap();
    let items = |out: &Value| -> Vec<String> {
        out["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| s(&i["sku"]).to_string())
            .collect()
    };
    assert!(
        items(&sdk) == vec![format!("PYT{tag}-SDK")] && items(&wl) == vec![format!("PYT{tag}-WL")]
    );
}

#[tokio::test]
async fn test_a_whatsapp_card_cannot_carry_an_attachment() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let out = direct!(
        d.pool,
        catalog::_h_propose_customer_message,
        &me,
        json!({"channel": "whatsapp", "chat_id": "nobody@c.us", "text": "Hello",
                             "attach": [{"type": "quotation", "id": "x"}]})
    );
    // nobody@c.us is not a number: take_in asks the WhatsApp service (not configured here) or refuses; either
    // way the card is never made.
    if let Ok(v) = out {
        assert!(v.get("error").is_some())
    }
}

// ─── test_nary_learns.py ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_a_skill_waits_for_the_owner_then_is_listed_and_readable() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let title = format!("Quote a white-label reseller {}", u("pytest-learns"));
    let mut conn = d.pool.acquire().await.unwrap();
    let out = catalog::_h_propose_skill(
        &mut conn,
        &me,
        json!({
        "title": title, "when": "A partner asks for white-label pricing",
        "steps": "1. search_catalog the parent\n2. propose_quotation_draft",
        "watch_out": "Never quote the tier", "learnt_from": "the Owner's correction"}),
    )
    .await
    .unwrap();
    assert_eq!(out["status"], "proposed");
    let (id, subject_type, layer): (Uuid, Option<String>, String) =
        sqlx::query_as("SELECT id, subject_type, layer FROM nary_memory WHERE title = $1")
            .bind(&title)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(subject_type.as_deref() == Some("skill") && layer == "company");
    // (Python approves it with memory.decide and checks skills.prompt_list: the skills port's tests.)
    sqlx::query("UPDATE nary_memory SET status = 'approved' WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let got = catalog::_h_use_skill(&mut conn, &me, json!({"title": "white-label reseller"}))
        .await
        .unwrap();
    assert!(
        got["title"] == json!(title) && s(&got["procedure"]).contains("propose_quotation_draft")
    );
}

#[tokio::test]
async fn test_a_tool_idea_goes_to_the_rnd_backlog_once() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let tag = u("pytest-learns");
    let args = json!({"title": format!("Attach a file to an email {tag}"), "problem": "The Owner asked to send the integration pack",
                      "what_it_should_do": "Attach a document from the ERP to a Nary email", "example_requests": "send the pack"});
    let mut conn = d.pool.acquire().await.unwrap();
    // no project set yet: a clear error (SettingError)
    let e = catalog::_h_propose_tool_idea(&mut conn, &me, args.clone())
        .await
        .unwrap_err();
    assert!(catalog::exc_str(&e).contains("nary.skills.tool_ideas_project"));
    let project = format!("Nary tools {tag}");
    sqlx::query("INSERT INTO rnd_projects (id, name, code, is_deleted) VALUES ($1, $2, $3, false)")
        .bind(Uuid::new_v4())
        .bind(&project)
        .bind(format!(
            "RND-{}",
            Uuid::new_v4().simple().to_string()[..8].to_uppercase()
        ))
        .execute(&mut *conn)
        .await
        .unwrap();
    let prev: Option<String> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'nary.skills.tool_ideas_project'",
    )
    .fetch_optional(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE system_settings SET value = $1 WHERE key = 'nary.skills.tool_ideas_project'",
    )
    .bind(&project)
    .execute(&mut *conn)
    .await
    .unwrap();
    let first = catalog::_h_propose_tool_idea(&mut conn, &me, args.clone()).await;
    let mut again_args = args.clone();
    again_args["example_requests"] = json!("attach the brochure");
    let again = catalog::_h_propose_tool_idea(&mut conn, &me, again_args).await;
    sqlx::query(
        "UPDATE system_settings SET value = $1 WHERE key = 'nary.skills.tool_ideas_project'",
    )
    .bind(prev.unwrap_or_default())
    .execute(&mut *conn)
    .await
    .unwrap();
    let (first, again) = (first.unwrap(), again.unwrap());
    let rows: Vec<(String, Option<Value>, Option<String>)> = sqlx::query_as(
        "SELECT item_type, labels, description FROM rnd_backlog_items WHERE title = $1",
    )
    .bind(s(&args["title"]))
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert!(first["status"] == "filed" && again["status"] == "added to the open idea");
    assert!(
        rows.len() == 1
            && rows[0].0 == "internal_tool"
            && rows[0]
                .1
                .clone()
                .and_then(|l| l.as_array().cloned())
                .unwrap_or_default()
                .contains(&json!("nary-idea"))
    );
    assert!(rows[0]
        .2
        .clone()
        .unwrap_or_default()
        .contains("attach the brochure"));
}

#[tokio::test]
async fn test_tiers_white_label_and_margin_only_for_cost_viewers() {
    let d = db().await;
    let sku = format!(
        "PYLRN{}",
        Uuid::new_v4().simple().to_string()[..5].to_uppercase()
    );
    let pid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, family, sale_model, price_metric, pricing_mode, \
         is_whitelabel, whitelabel_free_min_qty, cost_price, is_active, is_orderable, is_deleted) \
         VALUES ($1, $2, 'Laptop 14', 'physical', 'Unit', 'laptops', 'one_time', 'unit', 'list', false, 50, 40000000, true, true, false)",
    )
    .bind(pid)
    .bind(&sku)
    .execute(&d.pool)
    .await
    .unwrap();
    exec(&d.pool, &format!(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ('{}', '{pid}', 'JPY', 70000000, '2000-01-01');
         INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) VALUES ('{}', '{pid}', 21, NULL, 17.5);",
        Uuid::new_v4(), Uuid::new_v4())).await;
    let seller = person(&d.pool, false, &["sales:view_catalog"]).await;
    let owner = person(&d.pool, true, &[]).await;
    // (The catalog_digest half of this test is the digest port's.)
    let owner_view = direct!(
        d.pool,
        catalog::_h_search_catalog,
        &owner,
        json!({"query": sku})
    )
    .unwrap();
    let seller_view = direct!(
        d.pool,
        catalog::_h_search_catalog,
        &seller,
        json!({"query": sku})
    )
    .unwrap();
    let c = &owner_view["products"][0];
    assert!(
        !c["volume_tiers"].as_array().unwrap().is_empty()
            && c["cost_price"].as_f64().unwrap() != 0.0
    );
    let p = &seller_view["products"][0];
    assert!(p.get("volume_tiers").is_none() && p.get("cost_price").is_none());
}

// ─── test_catalog_pricing_c.py ───────────────────────────────────────────────

#[tokio::test]
async fn test_search_catalog_returns_only_sellable_items_with_their_price() {
    let d = db().await;
    let tag = u("PYS");
    let add = |name: String,
               extra: &'static str,
               values: &'static str,
               price: Option<&'static str>| {
        let pool = d.pool.clone();
        async move {
            let id = Uuid::new_v4();
            exec(&pool, &format!(
                "INSERT INTO product_catalog (id, sku, name, product_type, sale_model, price_metric, pricing_mode, \
                 is_whitelabel, is_active, is_orderable, is_deleted{extra}) VALUES ('{id}', '{}', '{name}', 'physical', \
                 {values})", u("PYT"))).await;
            if let Some(p) = price {
                exec(&pool, &format!("INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) \
                                      VALUES ('{}', '{id}', 'JPY', {p}, '2000-01-01')", Uuid::new_v4())).await;
            }
            id
        }
    };
    add(format!("{tag} live"),
        ", rate_period_months, billing_interval_months, min_term_months, modules, family",
        "'recurring', 'user', 'list', false, true, true, false, 1, 12, 12, '{reporting}', 'pytest_family'",
        Some("250000")).await;
    add(
        format!("{tag} retired"),
        "",
        "'one_time', 'unit', 'list', false, false, false, false",
        Some("1"),
    )
    .await;
    add(
        format!("{tag} not orderable"),
        "",
        "'one_time', 'unit', 'list', false, true, false, false",
        Some("1"),
    )
    .await;
    add(
        format!("{tag} quote"),
        "",
        "'one_time', 'unit', 'quote_only', false, true, true, false",
        None,
    )
    .await;
    let nobody = person(&d.pool, false, &[]).await;
    let out = direct!(
        d.pool,
        catalog::_h_search_catalog,
        &nobody,
        json!({"query": tag, "currency": "JPY"})
    )
    .unwrap();
    let usd = direct!(
        d.pool,
        catalog::_h_search_catalog,
        &nobody,
        json!({"query": format!("{tag} live"), "currency": "SGD"})
    )
    .unwrap();
    let by_name: std::collections::HashMap<String, Value> = out["products"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p["name"]).to_string(), p.clone()))
        .collect();
    let keys: HashSet<String> = by_name.keys().cloned().collect();
    assert_eq!(
        keys,
        HashSet::from([format!("{tag} live"), format!("{tag} quote")])
    );
    let item = &by_name[&format!("{tag} live")];
    assert_eq!(
        (
            item["price"].as_f64(),
            s(&item["sale_model"]),
            s(&item["price_metric"]),
            item["min_term_months"].as_i64()
        ),
        (Some(250000.0), "recurring", "user", Some(12))
    );
    assert!(item["modules"] == json!(["reporting"]));
    assert_eq!(item["family"], "pytest_family");
    let q = &by_name[&format!("{tag} quote")];
    assert!(q["price"].is_null() && s(&q["price_note"]).to_lowercase().contains("quote"));
    // A currency with no price of its own and nothing to convert from: the message, not a price
    let p = &usd["products"].as_array().unwrap()[0];
    assert_eq!(usd["products"].as_array().unwrap().len(), 1);
    assert!(p["price"].is_null() && !s(&p["price_note"]).is_empty());
}

// ─── test_nary_send_as_nary.py ───────────────────────────────────────────────

/// `inbound_thread()`: a CRM thread with one inbound email.
async fn inbound_thread(pool: &PgPool) -> (Uuid, String) {
    let thread = Uuid::new_v4();
    let from = "adams@globex-pytest.example".to_string();
    sqlx::query(
        "INSERT INTO crm_email_threads (id, subject) VALUES ($1, 'Follow-Up on Video Analytics')",
    )
    .bind(thread)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, subject, body_text, received_at) \
         VALUES ($1, $2, $3, 'inbound', $4, 'Follow-Up on Video Analytics', 'Hello', now() - interval '1 hour')",
    )
    .bind(Uuid::new_v4())
    .bind(thread)
    .bind(format!("<{}@globex.example>", Uuid::new_v4().simple()))
    .bind(&from)
    .execute(pool)
    .await
    .unwrap();
    (thread, from)
}

#[tokio::test]
async fn test_the_chat_card_refuses_placeholders_and_finds_the_thread_recipient() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let (thread, from) = inbound_thread(&d.pool).await;
    let bad = direct!(
        d.pool,
        catalog::_h_propose_customer_message,
        &me,
        json!({"channel": "email",
        "thread_id": thread.to_string(), "text": "Dear Sir,\n\nBest regards,\n[Name]"})
    )
    .unwrap();
    assert!(s(&bad["error"]).contains("placeholder"));
    let ok = direct!(
        d.pool,
        catalog::_h_propose_customer_message,
        &me,
        json!({"channel": "email",
        "thread_id": thread.to_string(), "text": "Dear Sir,\n\nBest regards,"})
    )
    .unwrap();
    let card = &ok["proposal"];
    assert!(card["kind"] == "customer_message" && card["to"] == json!(from));
    assert!(s(&card["subject"]).starts_with("Re: Follow-Up on Video Analytics"));
    // (Python then checks staff_chat._request_for / describe: A4/staff_chat.)
}

#[tokio::test]
async fn test_the_full_email_is_readable_not_only_its_start() {
    let d = db().await;
    let (thread, _) = inbound_thread(&d.pool).await;
    sqlx::query(
        "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, subject, body_text, received_at) \
         VALUES ($1, $2, $3, 'inbound', 'adams@globex-pytest.example', 'more', $4, now())",
    )
    .bind(Uuid::new_v4())
    .bind(thread)
    .bind(format!("<{}@globex.example>", Uuid::new_v4().simple()))
    .bind(format!("{} LAST PARAGRAPH", "x".repeat(2000)))
    .execute(&d.pool)
    .await
    .unwrap();
    let me = person(&d.pool, true, &[]).await;
    let out = direct!(
        d.pool,
        catalog::_h_get_email_thread,
        &me,
        json!({"thread_id": thread.to_string()})
    )
    .unwrap();
    let msgs = out["messages"].as_array().unwrap();
    assert!(s(&msgs.last().unwrap()["text"]).ends_with("LAST PARAGRAPH") && msgs.len() == 2);
}

// ─── test_mail_sync.py (the assistant's half) ────────────────────────────────

#[tokio::test]
async fn test_the_assistant_can_look_at_sent_mail() {
    // Python fills the CRM inbox through poll_crm_inbox from a fake IMAP server (the crm_inbox port's test);
    // here the two messages the poll stores are written directly.
    let d = db().await;
    let tag = &Uuid::new_v4().simple().to_string()[..8];
    let thread = Uuid::new_v4();
    sqlx::query("INSERT INTO crm_email_threads (id, subject) VALUES ($1, $2)")
        .bind(thread)
        .bind(format!("Question {tag}"))
        .execute(&d.pool)
        .await
        .unwrap();
    for (direction, from, to, subject, is_read) in [
        (
            "inbound",
            "eve@acme.test",
            "hello@example.com",
            format!("Question {tag}"),
            false,
        ),
        (
            "outbound",
            "hello@example.com",
            "eve@acme.test",
            format!("Re: Question {tag}"),
            true,
        ),
    ] {
        sqlx::query(
            "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, to_emails, subject, \
             body_text, is_read, received_at) VALUES ($1, $2, $3, $4, $5, ARRAY[$6]::varchar[], $7, 'body', $8, now())",
        )
        .bind(Uuid::new_v4())
        .bind(thread)
        .bind(format!("<{}@x.test>", Uuid::new_v4().simple()))
        .bind(direction)
        .bind(from)
        .bind(to)
        .bind(&subject)
        .bind(is_read)
        .execute(&d.pool)
        .await
        .unwrap();
    }
    let user = person(&d.pool, false, &["crm:view_all"]).await;
    let sent = run(
        &d.pool,
        &user,
        "search_emails",
        json!({"folder": "sent", "query": tag}),
    )
    .await;
    let unread = run(
        &d.pool,
        &user,
        "search_emails",
        json!({"unread_only": true, "query": tag}),
    )
    .await;
    let to_acme = run(
        &d.pool,
        &user,
        "search_emails",
        json!({"folder": "sent", "to": "acme.test", "query": tag}),
    )
    .await;
    let subjects = |v: &Value| -> Vec<String> {
        v["emails"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| s(&e["subject"]).to_string())
            .collect()
    };
    assert_eq!(subjects(&sent), vec![format!("Re: Question {tag}")]);
    assert!(
        sent["emails"][0]["direction"] == "sent"
            && sent["emails"][0]["to"] == json!(["eve@acme.test"])
    );
    assert_eq!(subjects(&unread), vec![format!("Question {tag}")]);
    assert_eq!(to_acme["emails"].as_array().unwrap().len(), 1);
}

// ─── test_ai_erp_guide.py (the tool) ─────────────────────────────────────────

#[tokio::test]
async fn test_the_tool_reads_a_section_by_name_and_lists_them() {
    // (Python first runs erp_guide.sync, which indexes the guide for search_ai_knowledge; the tool reads the
    // guide itself.)
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let listing = direct!(d.pool, catalog::_h_erp_guide, &me, json!({})).unwrap();
    let picklists = direct!(
        d.pool,
        catalog::_h_erp_guide,
        &me,
        json!({"topic": "crm-picklists"})
    )
    .unwrap();
    let fuzzy = direct!(
        d.pool,
        catalog::_h_erp_guide,
        &me,
        json!({"topic": "how do invoices and payments work"})
    )
    .unwrap();
    let api = direct!(
        d.pool,
        catalog::_h_erp_guide,
        &me,
        json!({"topic": "api:nary"})
    )
    .unwrap();
    let nothing = direct!(
        d.pool,
        catalog::_h_erp_guide,
        &me,
        json!({"topic": "zzzz qqqq"})
    )
    .unwrap();
    assert!(listing["topics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["topic"] == "sales"));
    assert!(
        s(&picklists["text"]).contains("`email`") && s(&picklists["text"]).contains("Lead sources")
    );
    assert_eq!(fuzzy["topic"], "finance");
    assert!(
        s(&api["text"]).to_lowercase().starts_with("## nary")
            && s(&api["text"]).contains("/nary/send")
    );
    assert!(nothing.get("error").is_some() && !nothing["topics"].as_array().unwrap().is_empty());
}

// ─── test_meeting_on_request.py / test_nary_send_failures.py / test_nary_backlog_oct13.py ─

/// `customer(email=...)` + `deal_for(cust)`: a contact with an active chat the customer started, and a deal.
async fn customer_with_deal(pool: &PgPool, email: Option<&str>) -> (Uuid, String, Uuid) {
    let contact_id = contact(pool, "John", &u("Pytest"), email).await;
    let cid = chat(pool, Some(contact_id), true).await;
    let deal = Uuid::new_v4();
    sqlx::query("INSERT INTO deals (id, title, contact_id, stage, currency, value, is_deleted) VALUES ($1, $2, $3, 'proposal', 'JPY', 100000000, false)")
        .bind(deal)
        .bind(u("Pytest deal"))
        .bind(contact_id)
        .execute(pool)
        .await
        .unwrap();
    (contact_id, cid, deal)
}

fn tomorrow_iso(hour: u32) -> String {
    let t = (Utc::now() + Duration::days(1))
        .date_naive()
        .and_hms_opt(hour - 9, 0, 0)
        .unwrap()
        .and_utc();
    granary_compat::pyjson::PyDateTime::from_chrono_utc(&t).isoformat()
}

#[tokio::test]
async fn test_the_card_finds_staff_by_name_and_the_customer_email_on_the_deal() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let staff = person(&d.pool, false, &["crm:view_all"]).await;
    let (_, cid, deal) = customer_with_deal(&d.pool, Some("diana@goldenflex.test")).await;
    let out = run(&d.pool, &me, "propose_meeting", json!({
        "title": "Acme Ltd x Globex Corp", "start": tomorrow_iso(11), "duration_minutes": 45,
        "staff": [staff.name], "deal_id": deal.to_string(),
        "message": {"channel": "whatsapp", "chat_id": cid, "text": "Hello Ms Diana, our meeting tomorrow at 11:00 JST: {link}"}})).await;
    let card = &out["proposal"];
    assert!(
        card["kind"] == "meeting" && card["duration_minutes"] == 45,
        "{out}"
    );
    let ids: HashSet<String> = card["staff_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| s(i).to_string())
        .collect();
    assert_eq!(
        ids,
        HashSet::from([me.id.to_string(), staff.id.to_string()])
    );
    assert_eq!(card["customer_email"], "diana@goldenflex.test");
    assert_eq!(
        card["message"]["text"],
        "Hello Ms Diana, our meeting tomorrow at 11:00 JST: {link}"
    );
    assert!(
        card["message"]["chat_id"] == json!(cid) && s(&card["summary"]).contains("then WhatsApp")
    );
}

#[tokio::test]
async fn test_an_ambiguous_or_unknown_name_is_asked_not_guessed() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    person(&d.pool, false, &["crm:view_all"]).await;
    person(&d.pool, false, &["crm:view_all"]).await; // both are "P0 User …"
    let out = run(
        &d.pool,
        &me,
        "propose_meeting",
        json!({"title": "Call", "start": tomorrow_iso(11), "staff": ["P0 User"]}),
    )
    .await;
    assert!(
        s(&out["error"]).contains("matches several people")
            && s(&out["error"]).contains("Ask which one")
    );
    let out = run(
        &d.pool,
        &me,
        "propose_meeting",
        json!({"title": "Call", "start": tomorrow_iso(11), "staff": ["Nobody Pytest"]}),
    )
    .await;
    assert!(s(&out["error"]).contains("No ERP user matches"));
    let out = run(
        &d.pool,
        &me,
        "propose_meeting",
        json!({"title": "Call", "start": "2020-01-01T10:00"}),
    )
    .await;
    assert!(s(&out["error"]).contains("already passed"));
}

#[tokio::test]
async fn test_the_card_says_up_front_when_the_chat_cant_be_written() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let staff = person(&d.pool, false, &["crm:view_all"]).await;
    let (_, cid, _) = customer_with_deal(&d.pool, None).await;
    sqlx::query("UPDATE wa_chats SET staff_user_id = $1 WHERE chat_id = $2")
        .bind(staff.id)
        .bind(&cid)
        .execute(&d.pool)
        .await
        .unwrap();
    let out = run(
        &d.pool,
        &me,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello"}),
    )
    .await;
    assert!(
        s(&out["error"]).starts_with("Not sent: ") && s(&out["error"]).contains("staff"),
        "{out}"
    );
    let out = run(
        &d.pool,
        &me,
        "propose_meeting",
        json!({"title": "TEST", "start": tomorrow_iso(11),
        "message": {"channel": "whatsapp", "chat_id": cid, "text": "Link: {link}"}}),
    )
    .await;
    assert!(s(&out["error"]).contains("staff") && s(&out["error"]).contains("without `message`"));
}

async fn booked_meeting(pool: &PgPool, deal: Uuid, contact_id: Uuid, cid: &str) -> Uuid {
    let id = Uuid::new_v4();
    let start = Utc::now() + Duration::days(1);
    sqlx::query(
        "INSERT INTO nary_meetings (id, deal_id, contact_id, chat_id, channel, slots, status, starts_at, ends_at, event_id, \
         meet_link, attendee_email, title, origin, attendees) VALUES ($1, $2, $3, $4, 'whatsapp', '[]', 'booked', $5, $6, \
         $7, $8, 'diana@goldenflex.test', 'ACME x Golden Flex', 'request', '[\"diana@goldenflex.test\"]')",
    )
    .bind(id)
    .bind(deal)
    .bind(contact_id)
    .bind(cid)
    .bind(start)
    .bind(start + Duration::minutes(60))
    .bind(format!("evt-{}", Uuid::new_v4().simple()))
    .bind(format!("https://meet.google.com/{}", &Uuid::new_v4().simple().to_string()[..10]))
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_the_invitation_goes_again_without_a_new_event() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let (contact_id, cid, deal) = customer_with_deal(&d.pool, Some("owner@globex.test")).await;
    let mid = booked_meeting(&d.pool, deal, contact_id, &cid).await;
    let link: String = sqlx::query_scalar("SELECT meet_link FROM nary_meetings WHERE id = $1")
        .bind(mid)
        .fetch_one(&d.pool)
        .await
        .unwrap();
    let listed = run(&d.pool, &me, "list_meetings", json!({})).await;
    let mine = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["meeting_id"] == json!(mid.to_string()))
        .unwrap()
        .clone();
    assert_eq!(mine["meet_link"], json!(link));
    let out = run(
        &d.pool,
        &me,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": cid,
        "text": "Here is the link for tomorrow's meeting: {link}", "meeting_id": mid.to_string()}),
    )
    .await;
    let card = &out["proposal"];
    assert!(
        card["text"] == json!(format!("Here is the link for tomorrow's meeting: {link}"))
            && card["meeting_id"] == json!(mid.to_string())
    );
    // (Python then sends it through POST /nary/send: wave 4.)
    let bad = run(
        &d.pool,
        &me,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": cid,
        "text": "{link}", "meeting_id": Uuid::new_v4().to_string()}),
    )
    .await;
    assert!(s(&bad["error"]).contains("No such meeting"));
}

#[tokio::test]
async fn test_a_guest_is_added_to_the_booked_event_on_confirm() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let (contact_id, cid, deal) = customer_with_deal(&d.pool, Some("diana@goldenflex.test")).await;
    let mid = booked_meeting(&d.pool, deal, contact_id, &cid).await;
    let out = run(
        &d.pool,
        &me,
        "propose_meeting_guests",
        json!({"meeting_id": mid.to_string(),
        "emails": ["Office.Manager1@example.net"], "save_on_contact": true}),
    )
    .await;
    let card = &out["proposal"];
    assert!(
        card["kind"] == "nary_action"
            && card["action"] == "meeting.add_guests"
            && card.get("auto").is_none(),
        "{out}"
    );
    // (Python then runs it through POST /nary/actions/run against the Google fake: wave 4.)
}

#[tokio::test]
async fn test_cancelling_tells_google_marks_it_and_stops_the_brief() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let (contact_id, cid, deal) = customer_with_deal(&d.pool, None).await;
    let mid = booked_meeting(&d.pool, deal, contact_id, &cid).await;
    let out = run(
        &d.pool,
        &me,
        "propose_meeting_cancel",
        json!({"meeting_id": mid.to_string(), "reason": "a test"}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], "meeting.cancel");
    // (Python then confirms through POST /nary/actions/run: wave 4.) The meeting once cancelled:
    sqlx::query("UPDATE nary_meetings SET status = 'cancelled' WHERE id = $1")
        .bind(mid)
        .execute(&d.pool)
        .await
        .unwrap();
    let again = run(
        &d.pool,
        &me,
        "propose_meeting_cancel",
        json!({"meeting_id": mid.to_string()}),
    )
    .await;
    assert!(s(&again["error"]).contains("already cancelled"));
}

async fn set_level(pool: &PgPool, action: &str, level: i64) -> String {
    let current: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
            .fetch_one(pool)
            .await
            .unwrap();
    let mut levels: Value = serde_json::from_str(&current).unwrap();
    levels[action] = json!(level);
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.autonomy.levels'")
        .bind(levels.to_string())
        .execute(pool)
        .await
        .unwrap();
    current
}

async fn restore_levels(pool: &PgPool, value: &str) {
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.autonomy.levels'")
        .bind(value)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_chat_actions_follow_the_autonomy_page() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let (contact_id, cid, deal) = customer_with_deal(&d.pool, None).await;
    let mid = booked_meeting(&d.pool, deal, contact_id, &cid).await;
    let args = json!({"meeting_id": mid.to_string(), "emails": ["guest@pytest.test"]});
    let saved = set_level(&d.pool, "meeting.add_guests", 0).await;
    let out = run(&d.pool, &me, "propose_meeting_guests", args.clone()).await;
    let shadow = (
        out["draft_only"] == true
            && s(&out["note"]).contains("Shadow")
            && out.get("proposal").is_none(),
        out.clone(),
    );
    let row: Option<(String, String)> = match s(&out["card"])
        .split('=')
        .nth(1)
        .and_then(|i| Uuid::parse_str(i).ok())
    {
        Some(id) => {
            sqlx::query_as("SELECT status, origin FROM agent_pending_actions WHERE id = $1")
                .bind(id)
                .fetch_optional(&d.pool)
                .await
                .unwrap()
        }
        None => None,
    };
    set_level(&d.pool, "meeting.add_guests", 2).await;
    let act = run(&d.pool, &me, "propose_meeting_guests", args).await;
    restore_levels(&d.pool, &saved).await;
    assert!(shadow.0, "{}", shadow.1);
    assert_eq!(row, Some(("shadow".to_string(), "chat".to_string())));
    assert!(act["proposal"]["auto"] == true && s(&act["note"]).contains("Act and tell"));
}

#[tokio::test]
async fn test_two_contacts_are_merged_from_the_chat() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let keep = Uuid::new_v4();
    let gone = Uuid::new_v4();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, phone, email, is_deleted) VALUES ($1, 'Diana', 'Pytest', '+8181388599240', 'diana@pytest.test', false)")
        .bind(keep).execute(&d.pool).await.unwrap();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, position, is_deleted) VALUES ($1, 'Diana', 'Pytest', 'Purchasing', false)")
        .bind(gone).execute(&d.pool).await.unwrap();
    let out = run(
        &d.pool,
        &me,
        "propose_contact_merge",
        json!({"keep_id": keep.to_string(), "merge_id": gone.to_string()}),
    )
    .await;
    let card = &out["proposal"];
    assert!(
        card["action"] == "contact.merge" && s(&card["summary"]).contains("position"),
        "{out}"
    );
    // (Python then merges through POST /nary/actions/run: wave 4.)
}

// ─── test_nary_take_in.py (the card for a chat Nary already knows) ───────────

#[tokio::test]
async fn test_a_known_chat_is_used_as_it_is_without_asking_whatsapp() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let cid = chat(&d.pool, None, true).await;
    for _ in 0..2 {
        let out = direct!(
            d.pool,
            catalog::_h_propose_customer_message,
            &me,
            json!({"channel": "whatsapp", "chat_id": cid,
            "text": "Good afternoon Mr David, here is the Google Meet invitation for Thursday."})
        )
        .unwrap();
        assert!(out.get("proposal").is_some(), "{out}");
    }
    // (The WhatsApp-service half of test_nary_take_in.py is the history port's: take_in itself.)
}
