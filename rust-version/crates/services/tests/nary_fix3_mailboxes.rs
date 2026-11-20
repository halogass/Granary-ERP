//! Nary batch 3, agent R2 (20 Oct 2026): Nary reads staff mailboxes, with permission rules.
//!
//! - a holder of email:read_staff_mailboxes searches and reads another staff member's mailbox (by name,
//!   by email, or "all"); every such read writes an audit row, and the owner is notified only when
//!   nary.mailboxes.notify_owner is "true";
//! - anyone else is refused with a clear message (search and read), and nothing is logged;
//! - everyone's own mailbox still works, and search_emails without `mailbox` is unchanged;
//! - "I sent you an email": the staff member's Sent folder, to the Owner;
//! - MCP: the connection lists the tools by the person's permissions and the same rules apply;
//! - the migration and the seed give the permission to the superadmin role; the schemas pass the
//!   Anthropic rules.

mod support;

use std::collections::HashSet;
use std::sync::OnceLock;

use granary_core::auth::AuthUser;
use granary_services::ai::schema;
use granary_services::ai::tools::catalog::{self, mailbox_tools, run_tool};
use serde_json::{json, Value};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

static URL: OnceLock<String> = OnceLock::new();
// every test sets the shared session pool (the audit rows are written in their own session): one at a time
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Db {
    pool: PgPool,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

async fn db() -> Db {
    let guard = LOCK.lock().await;
    if URL.get().is_none() {
        let pool = support::fresh_db("r2_fix3_mailboxes").await;
        pool.close().await;
        URL.set(std::env::var("DATABASE_URL").unwrap()).ok();
    }
    support::init();
    let pool = PgPool::connect(URL.get().unwrap()).await.expect("connect");
    granary_services::nary::set_session_pool(pool.clone());
    sqlx::query(
        "UPDATE system_settings SET value = 'false' WHERE key = 'nary.mailboxes.notify_owner'",
    )
    .execute(&pool)
    .await
    .unwrap();
    Db {
        pool,
        _guard: guard,
    }
}

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

const HOLDER: &[&str] = &["crm:view_all", mailbox_tools::READ_STAFF_MAILBOXES];

async fn person(pool: &PgPool, name: &str, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}-{}@granary.test", name.to_lowercase(), tag()))
    .bind(format!("{name} {}", tag()))
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

/// A connected mailbox for this person; returns the account id.
async fn account(pool: &PgPool, u: &AuthUser) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO email_accounts (id, user_id, email_address, imap_host, smtp_host, username, password_encrypted, \
         is_active) VALUES ($1, $2, $3, 'imap.x', 'smtp.x', $3, 'x', true)",
    )
    .bind(id)
    .bind(u.id)
    .bind(&u.email)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[allow(clippy::too_many_arguments)]
async fn mail(
    pool: &PgPool,
    account: Uuid,
    folder: &str,
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
    thread: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO email_messages (id, account_id, message_id, folder, from_address, to_addresses, subject, \
         body_text, thread_id, is_read, is_deleted, date_received, attachments) \
         VALUES ($1, $2, $3, $4, $5, ARRAY[$6]::varchar[], $7, $8, $9, false, false, now(), \
         '[{\"filename\": \"spec.pdf\"}]'::jsonb)",
    )
    .bind(id)
    .bind(account)
    .bind(format!("<{}@x.test>", Uuid::new_v4().simple()))
    .bind(folder)
    .bind(from)
    .bind(to)
    .bind(subject)
    .bind(body)
    .bind(thread)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// A tool call in a transaction that is rolled back (as the chat runs them).
async fn run(pool: &PgPool, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let out = run_tool(&mut tx, user, name, args).await;
    tx.rollback().await.unwrap();
    out
}

async fn audits(pool: &PgPool, reader: &AuthUser) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT entity_id, coalesce(entity_label, ''), new_values::text FROM audit_logs \
         WHERE user_id = $1 AND action = 'nary_mailbox_read' ORDER BY created_at",
    )
    .bind(reader.id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn notifications(pool: &PgPool, owner: &AuthUser) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND type = 'nary_mailbox_read'",
    )
    .bind(owner.id)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn subjects(v: &Value) -> Vec<String> {
    v["emails"]
        .as_array()
        .unwrap_or_else(|| panic!("{v}"))
        .iter()
        .map(|e| e["subject"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test]
async fn a_holder_searches_and_reads_another_staff_mailbox_and_it_is_audited() {
    let d = db().await;
    let t = tag();
    let owner = person(&d.pool, "Owner", HOLDER).await;
    let bob = person(&d.pool, "Bob", &["crm:view_own"]).await;
    let acc = account(&d.pool, &bob).await;
    let first = mail(
        &d.pool,
        acc,
        "INBOX",
        "client@acme.test",
        &bob.email,
        &format!("Spec {t}"),
        "Please see the spec.",
        &format!("thr-{t}"),
    )
    .await;
    let reply = mail(
        &d.pool,
        acc,
        "SENT",
        &bob.email,
        "client@acme.test",
        &format!("Re: Spec {t}"),
        "We will check and revert.",
        &format!("thr-{t}"),
    )
    .await;

    // by name
    let out = run(
        &d.pool,
        &owner,
        "search_emails",
        json!({"mailbox": bob.name, "query": t}),
    )
    .await;
    assert!(out.get("error").is_none(), "{out}");
    let mut got = subjects(&out);
    got.sort();
    assert_eq!(got, vec![format!("Re: Spec {t}"), format!("Spec {t}")]);
    assert_eq!(out["mailbox"], json!(bob.email));
    assert!(out["notice"]
        .as_str()
        .unwrap()
        .contains("only for the person who asked"));
    assert!(out["emails"][0]["email_id"].is_string());
    let rows = audits(&d.pool, &owner).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, bob.id.to_string());
    assert!(rows[0].1.contains(&bob.email));
    assert!(
        rows[0].2.contains(&t) && rows[0].2.contains(&first.to_string()),
        "{}",
        rows[0].2
    );

    // by email, read in full: the whole conversation, attachment names
    let by_email = run(
        &d.pool,
        &owner,
        "search_emails",
        json!({"mailbox": bob.email.to_uppercase(), "query": t}),
    )
    .await;
    assert_eq!(subjects(&by_email).len(), 2);
    let full = run(
        &d.pool,
        &owner,
        "get_mailbox_email",
        json!({"email_id": first.to_string()}),
    )
    .await;
    assert!(full.get("error").is_none(), "{full}");
    let msgs = full["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["text"], json!("Please see the spec."));
    assert_eq!(msgs[0]["attachments"], json!(["spec.pdf"]));
    assert_eq!(msgs[1]["direction"], json!("sent"));
    let rows = audits(&d.pool, &owner).await;
    assert_eq!(rows.len(), 3);
    assert!(
        rows[2].2.contains(&reply.to_string()) && rows[2].2.contains("\"read\""),
        "{}",
        rows[2].2
    );
    // notify_owner is off by default: nothing for Bob
    assert_eq!(notifications(&d.pool, &bob).await, 0);
}

#[tokio::test]
async fn others_are_refused_with_a_clear_message_and_nothing_is_logged() {
    let d = db().await;
    let t = tag();
    let rachel = person(&d.pool, "Rachel", &["crm:view_all"]).await;
    let bob = person(&d.pool, "Bob", &[]).await;
    let acc = account(&d.pool, &bob).await;
    let id = mail(
        &d.pool,
        acc,
        "INBOX",
        "x@acme.test",
        &bob.email,
        &format!("Private {t}"),
        "secret",
        "",
    )
    .await;
    for args in [
        json!({"mailbox": bob.email, "query": t}),
        json!({"mailbox": "all", "query": t}),
    ] {
        let out = run(&d.pool, &rachel, "search_emails", args).await;
        let e = out["error"].as_str().unwrap_or_else(|| panic!("{out}"));
        assert!(
            e.contains("only have Nary read your own mailbox")
                && e.contains("email:read_staff_mailboxes"),
            "{e}"
        );
    }
    let out = run(
        &d.pool,
        &rachel,
        "get_mailbox_email",
        json!({"email_id": id.to_string()}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("email:read_staff_mailboxes"),
        "{out}"
    );
    assert!(!out.to_string().contains("secret"));
    assert!(audits(&d.pool, &rachel).await.is_empty());
    // unknown and ambiguous names are explained, not guessed
    let out = run(
        &d.pool,
        &rachel,
        "search_emails",
        json!({"mailbox": format!("nobody-{t}")}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("No staff member matches"),
        "{out}"
    );
}

#[tokio::test]
async fn ambiguous_names_ask_which_one() {
    let d = db().await;
    let owner = person(&d.pool, "Owner", HOLDER).await;
    let t = tag();
    for _ in 0..2 {
        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
             VALUES ($1, $2, $3, 'x', true, false, false, 'internal')",
        )
        .bind(Uuid::new_v4())
        .bind(format!("debbie-{}@granary.test", tag()))
        .bind(format!("Debbie {t}"))
        .execute(&d.pool)
        .await
        .unwrap();
    }
    let out = run(
        &d.pool,
        &owner,
        "search_emails",
        json!({"mailbox": format!("Debbie {t}")}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("Several staff members match"),
        "{out}"
    );
    assert!(audits(&d.pool, &owner).await.is_empty());
}

#[tokio::test]
async fn everyone_keeps_their_own_mailbox_and_the_default_search_is_unchanged() {
    let d = db().await;
    let t = tag();
    let bob = person(&d.pool, "Bob", &[]).await;
    let acc = account(&d.pool, &bob).await;
    let id = mail(
        &d.pool,
        acc,
        "INBOX",
        "c@acme.test",
        &bob.email,
        &format!("Mine {t}"),
        "hello there",
        "",
    )
    .await;
    for mb in ["me", bob.email.as_str(), bob.name.as_str()] {
        let out = run(
            &d.pool,
            &bob,
            "search_emails",
            json!({"mailbox": mb, "query": t}),
        )
        .await;
        assert_eq!(subjects(&out), vec![format!("Mine {t}")], "{mb}: {out}");
        assert!(out.get("notice").is_none());
    }
    let full = run(
        &d.pool,
        &bob,
        "get_mailbox_email",
        json!({"email_id": id.to_string()}),
    )
    .await;
    assert_eq!(full["messages"][0]["text"], json!("hello there"), "{full}");
    assert!(full.get("notice").is_none());
    // without mailbox: exactly the old result (own mailbox, no email_id), and no audit for own reads
    let old = run(&d.pool, &bob, "search_emails", json!({"query": t})).await;
    assert_eq!(subjects(&old), vec![format!("Mine {t}")]);
    assert!(old["emails"][0].get("email_id").is_none());
    assert_eq!(old["emails"][0]["source"], json!("personal"));
    assert!(audits(&d.pool, &bob).await.is_empty());
}

#[tokio::test]
async fn i_sent_you_an_email_is_found_in_the_staff_sent_folder() {
    let d = db().await;
    let t = tag();
    let owner = person(&d.pool, "Owner", HOLDER).await;
    let bob = person(&d.pool, "Bob", &[]).await;
    let acc = account(&d.pool, &bob).await;
    mail(
        &d.pool,
        acc,
        "SENT",
        &bob.email,
        &owner.email,
        &format!("Report {t}"),
        "the report",
        "",
    )
    .await;
    mail(
        &d.pool,
        acc,
        "SENT",
        &bob.email,
        "other@acme.test",
        &format!("Other {t}"),
        "x",
        "",
    )
    .await;
    mail(
        &d.pool,
        acc,
        "INBOX",
        &owner.email,
        &bob.email,
        &format!("Ask {t}"),
        "x",
        "",
    )
    .await;
    let out = run(
        &d.pool,
        &owner,
        "search_emails",
        json!({"mailbox": bob.name, "folder": "sent", "to": owner.email, "days": 7}),
    )
    .await;
    assert_eq!(subjects(&out), vec![format!("Report {t}")], "{out}");
    assert_eq!(out["emails"][0]["direction"], json!("sent"));
}

#[tokio::test]
async fn all_mailboxes_and_the_owner_notice_setting() {
    let d = db().await;
    let t = tag();
    let owner = person(&d.pool, "Owner", HOLDER).await;
    let bob = person(&d.pool, "Bob", &[]).await;
    let sarah = person(&d.pool, "Sarah", &[]).await;
    let a1 = account(&d.pool, &bob).await;
    let a2 = account(&d.pool, &sarah).await;
    mail(
        &d.pool,
        a1,
        "INBOX",
        "c@acme.test",
        &bob.email,
        &format!("PO {t} one"),
        "x",
        "",
    )
    .await;
    mail(
        &d.pool,
        a2,
        "INBOX",
        "c@acme.test",
        &sarah.email,
        &format!("PO {t} two"),
        "x",
        "",
    )
    .await;
    sqlx::query(
        "UPDATE system_settings SET value = 'true' WHERE key = 'nary.mailboxes.notify_owner'",
    )
    .execute(&d.pool)
    .await
    .unwrap();
    let out = run(
        &d.pool,
        &owner,
        "search_emails",
        json!({"mailbox": "all", "query": t}),
    )
    .await;
    assert_eq!(subjects(&out).len(), 2, "{out}");
    let rows = audits(&d.pool, &owner).await;
    let owners: HashSet<String> = rows.iter().map(|r| r.0.clone()).collect();
    assert_eq!(
        owners,
        HashSet::from(["all".to_string(), bob.id.to_string(), sarah.id.to_string()])
    );
    assert_eq!(notifications(&d.pool, &bob).await, 1);
    assert_eq!(notifications(&d.pool, &sarah).await, 1);
    assert_eq!(notifications(&d.pool, &owner).await, 0);
    sqlx::query(
        "UPDATE system_settings SET value = 'false' WHERE key = 'nary.mailboxes.notify_owner'",
    )
    .execute(&d.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn mcp_lists_the_tools_by_permission_and_applies_the_same_rules() {
    use granary_services::ai::mcp_access;
    let d = db().await;
    let t = tag();
    let rachel = person(&d.pool, "Rachel", &["crm:view_all"]).await;
    let owner = person(&d.pool, "Owner", HOLDER).await;
    let bob = person(&d.pool, "Bob", &[]).await;
    let acc = account(&d.pool, &bob).await;
    let id = mail(
        &d.pool,
        acc,
        "INBOX",
        "c@acme.test",
        &bob.email,
        &format!("Mcp {t}"),
        "body",
        "",
    )
    .await;
    let mut conn = d.pool.acquire().await.unwrap();
    for u in [&rachel, &owner] {
        let names: Vec<String> = mcp_access::allowed_tools(&mut conn, u, &["read".to_string()])
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(
            names.contains(&"search_emails".to_string())
                && names.contains(&"get_mailbox_email".to_string())
        );
    }
    drop(conn);
    let refused = run(
        &d.pool,
        &rachel,
        "get_mailbox_email",
        json!({"email_id": id.to_string()}),
    )
    .await;
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("email:read_staff_mailboxes"));
    let ok = run(
        &d.pool,
        &owner,
        "get_mailbox_email",
        json!({"email_id": id.to_string()}),
    )
    .await;
    assert_eq!(ok["messages"][0]["subject"], json!(format!("Mcp {t}")));
    assert_eq!(audits(&d.pool, &owner).await.len(), 1);
    // a read tool: a read-only connection keeps it
    assert!(!mcp_access::PROPOSAL_TOOLS.contains(&"get_mailbox_email"));
}

#[tokio::test]
async fn the_superadmin_role_holds_the_permission_and_the_setting_is_seeded() {
    let d = db().await;
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT r.slug FROM roles r JOIN role_permissions rpm ON rpm.role_id = r.id \
         JOIN permissions p ON p.id = rpm.permission_id WHERE p.code = 'email:read_staff_mailboxes' ORDER BY 1",
    )
    .fetch_all(&d.pool)
    .await
    .unwrap();
    assert_eq!(roles, vec!["superadmin".to_string()]);
    let v: Option<String> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'nary.mailboxes.notify_owner'",
    )
    .fetch_optional(&d.pool)
    .await
    .unwrap();
    assert!(v.is_some());
}

#[test]
fn the_schemas_pass_the_anthropic_rules() {
    for name in ["search_emails", "get_mailbox_email"] {
        let t = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        let sent = schema::for_anthropic(&t.parameters);
        assert!(
            schema::violations(&sent).is_empty(),
            "{name}: {:?}",
            schema::violations(&sent)
        );
    }
    let se = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "search_emails")
        .unwrap();
    assert!(se.parameters["properties"]["mailbox"].is_object());
}
