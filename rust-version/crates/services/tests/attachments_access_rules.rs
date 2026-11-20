//! Port of backend/tests/test_attachment_access.py (the service-level tests).
//!
//! Attachments are only as private as the record they hang off: `assert_can_access` is the one rule.
//!
//! Python note: the five project tests (portal user, not on the project, PM, team member, view_all) FAIL
//! on Python at main bddbe04, in their fixture, not in the service: `_project()` builds `Project(name=...)`
//! without `project_type`, which is NOT NULL, so the insert raises IntegrityError before the rule is ever
//! asked. The ports below give the project a `project_type` so the rule itself is exercised; the service
//! behaviour they assert is what Python's rule does once a project exists.
//!
//! The three endpoint tests (download/list through the router, the response shape of `_attachment_dict`)
//! belong to the attachments router port, not to this module.

mod support;

use granary_core::auth::{load_active_user, query_permissions, AuthUser};
use granary_core::errors::ApiError;
use granary_services::attachments_access::assert_can_access;
use sqlx::PgConnection;
use tokio::sync::OnceCell;
use uuid::Uuid;

static DB_URL: OnceCell<String> = OnceCell::const_new();

async fn conn() -> PgConnection {
    use sqlx::Connection;
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("s7_attachments").await;
            pool.close().await;
            std::env::var("DATABASE_URL").unwrap()
        })
        .await;
    PgConnection::connect(url).await.unwrap()
}

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// `make_person(perms=...)`: an internal user holding exactly `perms` through a role of its own.
async fn user(perms: &[&str]) -> AuthUser {
    let mut db = conn().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Pytest Person', 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("pytest.approver")))
    .execute(&mut db)
    .await
    .unwrap();
    if !perms.is_empty() {
        let role = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $3, false)",
        )
        .bind(role)
        .bind(u("pytest-perms"))
        .bind(u("pytest_perms"))
        .execute(&mut db)
        .await
        .unwrap();
        for code in perms {
            let pid: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM permissions WHERE code = $1")
                    .bind(code)
                    .fetch_optional(&mut db)
                    .await
                    .unwrap();
            let pid = match pid {
                Some(p) => p,
                None => {
                    let p = Uuid::new_v4();
                    sqlx::query(
                        "INSERT INTO permissions (id, code, name, module) VALUES ($1, $2, $2, $3)",
                    )
                    .bind(p)
                    .bind(code)
                    .bind(code.split(':').next().unwrap())
                    .execute(&mut db)
                    .await
                    .unwrap();
                    p
                }
            };
            sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
                .bind(role)
                .bind(pid)
                .execute(&mut db)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(id)
            .bind(role)
            .execute(&mut db)
            .await
            .unwrap();
    }
    load(&mut db, id).await
}

async fn load(db: &mut PgConnection, id: Uuid) -> AuthUser {
    let mut user = load_active_user(db, id).await.unwrap().unwrap();
    user.perms = query_permissions(db, id).await.unwrap();
    user
}

async fn portal_user() -> AuthUser {
    let mut db = conn().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Vendor Portal', 'x', true, false, false, 'vendor')",
    )
    .bind(id)
    .bind(format!("{}@outside.test", u("vendor")))
    .execute(&mut db)
    .await
    .unwrap();
    load(&mut db, id).await
}

async fn deal(owner: Uuid) -> Uuid {
    let mut db = conn().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, assigned_to_id, created_by_id, is_deleted) \
         VALUES ($1, $2, 'qualifying', $3, $3, false)",
    )
    .bind(id)
    .bind(u("pytest"))
    .bind(owner)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

/// `_project(**kwargs)`, with a project_type (see the module docs).
async fn project(pm_id: Option<Uuid>) -> Uuid {
    let mut db = conn().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO projects (id, name, project_type, status, currency, cost_to_date, objectives, deliverables, \
         pm_id, is_deleted) VALUES ($1, $2, 'implementation', 'planning', 'USD', 0, '[]', '[]', $3, false)",
    )
    .bind(id)
    .bind(u("pytest"))
    .bind(pm_id)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

async fn article(is_private: bool, created_by: Option<Uuid>, access_level: &str) -> Uuid {
    let mut db = conn().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO kb_articles (id, title, slug, status, is_private, created_by_id, access_level, is_deleted) \
         VALUES ($1, $2, $3, 'published', $4, $5, $6, false)",
    )
    .bind(id)
    .bind(u("pytest"))
    .bind(u("pytest"))
    .bind(is_private)
    .bind(created_by)
    .bind(access_level)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

async fn check(user: &AuthUser, entity_type: &str, entity_id: Uuid) -> bool {
    let mut db = conn().await;
    assert_can_access(&mut db, user, entity_type, &entity_id.to_string())
        .await
        .unwrap()
        .is_some()
}

async fn denied(user: &AuthUser, entity_type: &str, entity_id: Uuid) -> u16 {
    let mut db = conn().await;
    match assert_can_access(&mut db, user, entity_type, &entity_id.to_string()).await {
        Err(e @ ApiError::Http { .. }) => e.status().as_u16(),
        other => panic!("expected an HTTPException, got {other:?}"),
    }
}

// ─── CRM records: owner scoping ──────────────────────────────────────────────

#[tokio::test]
async fn test_view_own_may_not_read_another_reps_deal_attachments() {
    let mine = user(&["crm:view_own"]).await;
    let theirs = user(&["crm:view_own"]).await;
    let d = deal(theirs.id).await;
    assert_eq!(denied(&mine, "crm_deal", d).await, 403);
}

#[tokio::test]
async fn test_view_own_may_read_its_own_deal_attachments() {
    let mine = user(&["crm:view_own"]).await;
    let d = deal(mine.id).await;
    assert!(check(&mine, "crm_deal", d).await);
}

#[tokio::test]
async fn test_view_all_reads_any_deal_attachments() {
    let theirs = user(&["crm:view_own"]).await;
    let everyone = user(&["crm:view_all"]).await;
    let d = deal(theirs.id).await;
    assert!(check(&everyone, "crm_deal", d).await);
}

#[tokio::test]
async fn test_no_crm_permission_at_all_is_refused() {
    let outsider = user(&[]).await;
    let d = deal(user(&["crm:view_own"]).await.id).await;
    assert_eq!(denied(&outsider, "crm_deal", d).await, 403);
}

// ─── Portal users are out entirely ───────────────────────────────────────────

#[tokio::test]
async fn test_a_portal_user_is_refused_even_for_an_unowned_record() {
    let portal = portal_user().await;
    let p = project(None).await;
    assert_eq!(denied(&portal, "project", p).await, 403);
}

// ─── Projects: membership ────────────────────────────────────────────────────

#[tokio::test]
async fn test_someone_not_on_the_project_is_refused() {
    let outsider = user(&["projects:view_own"]).await;
    assert_eq!(denied(&outsider, "project", project(None).await).await, 403);
}

#[tokio::test]
async fn test_the_project_manager_may_read_it() {
    let pm = user(&["projects:view_own"]).await;
    let p = project(Some(pm.id)).await;
    assert!(check(&pm, "project", p).await);
}

#[tokio::test]
async fn test_a_team_member_may_read_it() {
    let member = user(&["projects:view_own"]).await;
    let p = project(None).await;
    let mut db = conn().await;
    sqlx::query("INSERT INTO project_members (id, project_id, user_id) VALUES ($1, $2, $3)")
        .bind(Uuid::new_v4())
        .bind(p)
        .bind(member.id)
        .execute(&mut db)
        .await
        .unwrap();
    assert!(check(&member, "project", p).await);
}

#[tokio::test]
async fn test_projects_view_all_skips_the_membership_check() {
    let boss = user(&["projects:view_all"]).await;
    assert!(check(&boss, "project", project(None).await).await);
}

// ─── Knowledge base: private + access_level ──────────────────────────────────

#[tokio::test]
async fn test_a_private_article_is_refused_to_everyone_but_its_author() {
    let author = user(&["kb:view"]).await;
    let other = user(&["kb:view"]).await;
    let a = article(true, Some(author.id), "all_staff").await;
    assert_eq!(denied(&other, "kb_article", a).await, 403);
    assert!(check(&author, "kb_article", a).await);
}

#[tokio::test]
async fn test_a_restricted_article_needs_kb_view_restricted() {
    let plain = user(&["kb:view"]).await;
    let privileged = user(&["kb:view", "kb:view_restricted"]).await;
    let a = article(false, None, "management").await;
    assert_eq!(denied(&plain, "kb_article", a).await, 403);
    assert!(check(&privileged, "kb_article", a).await);
}

#[tokio::test]
async fn test_an_all_staff_article_is_open_to_staff() {
    let plain = user(&["kb:view"]).await;
    assert!(
        check(
            &plain,
            "kb_article",
            article(false, None, "all_staff").await
        )
        .await
    );
}

/// Batch E: kb:view (seeded on every staff role) is the knowledge base read permission; an editing
/// permission includes reading.
#[tokio::test]
async fn test_kb_attachments_need_kb_view() {
    let a = article(false, None, "all_staff").await;
    assert_eq!(denied(&user(&[]).await, "kb_article", a).await, 403);
    assert!(check(&user(&["kb:view"]).await, "kb_article", a).await);
    assert!(check(&user(&["kb:create"]).await, "kb_article", a).await);
}

// ─── Unknown / missing ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_an_entity_type_with_no_rule_is_refused_not_waved_through() {
    let usr = user(&["crm:view_all"]).await;
    assert_eq!(denied(&usr, "purchase_order", Uuid::new_v4()).await, 403);
}

#[tokio::test]
async fn test_a_missing_record_is_a_404() {
    let usr = user(&["crm:view_all"]).await;
    assert_eq!(denied(&usr, "crm_deal", Uuid::new_v4()).await, 404);
}

// ─── Beyond the Python file: the exact texts and the remaining rules ─────────

async fn detail(user: &AuthUser, entity_type: &str, entity_id: &str) -> (u16, String) {
    let mut db = conn().await;
    match assert_can_access(&mut db, user, entity_type, entity_id).await {
        Err(ApiError::Http { status, detail, .. }) => {
            (status.as_u16(), detail.as_str().unwrap().to_string())
        }
        other => panic!("expected an HTTPException, got {other:?}"),
    }
}

#[tokio::test]
async fn details_match_python() {
    let none = user(&[]).await;
    let own = user(&["crm:view_own"]).await;
    let d = deal(user(&[]).await.id).await.to_string();
    assert_eq!(
        detail(&none, "crm_deal", &d).await,
        (403, "Permission required: crm:view_own".into())
    );
    assert_eq!(
        detail(&own, "crm_deal", &d).await,
        (403, "This record is assigned to someone else".into())
    );
    assert_eq!(
        detail(&own, "crm_deal", "not-a-uuid").await,
        (404, "Record not found".into())
    );
    // uuid.UUID(str(...)) acceptance: braces and "urn:uuid:" are fine
    assert_eq!(
        detail(&own, "crm_deal", &format!("{{{d}}}")).await,
        (403, "This record is assigned to someone else".into())
    );
    assert_eq!(
        detail(&own, "purchase_order", "x").await,
        (
            403,
            "Attachments on 'purchase_order' are not readable through this API".into()
        )
    );
    assert_eq!(
        detail(&none, "contract", "x").await,
        (404, "Record not found".into())
    );
    assert_eq!(
        detail(&none, "project", &project(None).await.to_string()).await,
        (403, "You are not on this project".into())
    );
    let portal = portal_user().await;
    assert_eq!(
        detail(&portal, "crm_deal", &d).await,
        (403, "Internal users only".into())
    );
    let a = article(true, None, "all_staff").await.to_string();
    assert_eq!(
        detail(&none, "kb_article", &a).await,
        (403, "Permission required: kb:view".into())
    );
    let reader = user(&["kb:view"]).await;
    assert_eq!(
        detail(&reader, "kb_article", &a).await,
        (403, "This article is private".into())
    );
    let a = article(false, None, "management").await.to_string();
    assert_eq!(
        detail(&reader, "kb_article", &a).await,
        (403, "Permission required: kb:view_restricted".into())
    );
}

#[tokio::test]
async fn unowned_crm_records_and_soft_deleted_ones() {
    let own = user(&["crm:view_own"]).await;
    let mut db = conn().await;
    // An unassigned organisation is visible to crm:view_own.
    let org = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted) VALUES ($1, 'Pytest Org', false)",
    )
    .bind(org)
    .execute(&mut db)
    .await
    .unwrap();
    assert!(check(&own, "crm_org", org).await);
    // A soft-deleted deal is a 404, even for its owner.
    let d = deal(own.id).await;
    sqlx::query("UPDATE deals SET is_deleted = true WHERE id = $1")
        .bind(d)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(denied(&own, "crm_deal", d).await, 404);
}

#[tokio::test]
async fn rnd_projects_and_contracts() {
    let mut db = conn().await;
    let lead = user(&[]).await;
    let other = user(&[]).await;
    let viewer = user(&["rnd:manage"]).await;
    let rnd = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rnd_projects (id, name, code, lead_engineer_id, is_deleted) VALUES ($1, $2, $3, $4, false)",
    )
    .bind(rnd)
    .bind(u("pytest"))
    .bind(u("RD"))
    .bind(lead.id)
    .execute(&mut db)
    .await
    .unwrap();
    assert!(check(&lead, "rnd_project", rnd).await);
    assert!(check(&viewer, "rnd_project", rnd).await);
    assert_eq!(
        detail(&other, "rnd_project", &rnd.to_string()).await,
        (403, "Permission required: rnd:view".into())
    );

    // Superadmins pass (and get the record); an unknown type gives them None.
    let sa_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin LIMIT 1")
        .fetch_one(&mut db)
        .await
        .unwrap();
    let sa = load(&mut db, sa_id).await;
    assert!(check(&sa, "rnd_project", rnd).await);
    assert!(assert_can_access(&mut db, &sa, "purchase_order", "x")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        detail(&sa, "crm_deal", &Uuid::new_v4().to_string()).await,
        (404, "Record not found".into())
    );
}
