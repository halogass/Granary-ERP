//! Nary fix batch 2, agent T2 (Owner review 14-19 Oct): CRM hygiene from the chat.
//!
//! - A2  find_duplicate_organizations finds the five groups left on prod (Contoso x3, Fabrikam, Initrode Kobe,
//!   XYZ, Umbrella); propose_organization_merge makes one card for a multi-merge that lists, per merged copy, what
//!   moves and which fields it fills; applied, nothing is lost: every row that pointed at a merged copy
//!   (every foreign key in the schema plus activities, files, reminders, Nary's notes, research) points at
//!   the one kept;
//! - A4  list_nary_cards: drafted vs final text with the changes, who decided, the reject reason, filters,
//!   paging, Owner / Nary admin only;
//! - A9  propose_contact_delete: Owner only, refused with open deals unless they are listed on the card, the
//!   CRM's soft delete when applied.

mod support;

use std::collections::HashSet;
use std::sync::OnceLock;

use granary_core::auth::AuthUser;
use granary_services::ai::schema;
use granary_services::ai::tools::catalog::{self, run_tool};
use granary_services::crm::dedupe;
use granary_services::nary::{cards, executor, hygiene};
use serde_json::{json, Value};
use sqlx::{Connection, PgPool};
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
        let pool = support::fresh_db("t2_fix2_hygiene").await;
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

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

async fn person(pool: &PgPool, superadmin: bool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, $4, false, 'internal')",
    )
    .bind(id)
    .bind(format!("t2-{}@granary.test", tag()))
    .bind(format!("T2 User {}", tag()))
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

/// A tool call in a transaction that is rolled back (as the chat runs them).
async fn run(pool: &PgPool, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let out = run_tool(&mut tx, user, name, args).await;
    tx.rollback().await.unwrap();
    out
}

async fn exec(pool: &PgPool, sql: &str, ids: &[Uuid]) {
    let mut q = sqlx::query(sql);
    for id in ids {
        q = q.bind(*id);
    }
    q.execute(pool).await.unwrap();
}

async fn org(pool: &PgPool, name: &str, phone: Option<&str>, city: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, phone, city, is_deleted, custom) VALUES ($1, $2, $3, $4, false, '{}')",
    )
    .bind(id)
    .bind(name)
    .bind(phone)
    .bind(city)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

// ─── A2: organization duplicates ─────────────────────────────────────────────

/// Every foreign key to organizations in the schema is moved by a merge (dedupe::related).
#[tokio::test]
async fn every_foreign_key_to_organizations_is_moved_by_a_merge() {
    let d = db().await;
    let fks: Vec<(String, String)> = sqlx::query_as(
        "SELECT kcu.table_name::text, kcu.column_name::text \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
         JOIN information_schema.constraint_column_usage ccu \
           ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema \
         WHERE tc.constraint_type = 'FOREIGN KEY' AND ccu.table_name = 'organizations' \
           AND ccu.column_name = 'id' ORDER BY 1, 2",
    )
    .fetch_all(&d.pool)
    .await
    .unwrap();
    assert!(fks.len() >= 15, "{fks:?}");
    let moved: HashSet<(String, String)> = dedupe::related("organization")
        .iter()
        .map(|(t, c, _)| (t.to_string(), c.to_string()))
        .collect();
    let missing: Vec<&(String, String)> = fks.iter().filter(|fk| !moved.contains(*fk)).collect();
    assert!(missing.is_empty(), "not moved by a merge: {missing:?}");
    // and the typed references exist as written
    for (table, tcol, icol, _, _) in dedupe::related_typed("organization") {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM information_schema.columns WHERE table_name = $1 AND column_name IN ($2, $3)",
        )
        .bind(table)
        .bind(tcol)
        .bind(icol)
        .fetch_one(&d.pool)
        .await
        .unwrap();
        assert_eq!(n, 2, "{table}.{tcol}/{icol}");
    }
}

#[test]
fn the_name_key_ignores_legal_forms_case_punctuation_and_spaces() {
    let k = hygiene::org_key;
    assert_eq!(
        k("CONTOSO PACIFIC PARTNERS LTD."),
        k("Contoso Pacific Partners, Inc")
    );
    assert_eq!(k("FabriKam Japan Ltd"), k("Fabri Kam Japan"));
    assert_eq!(k("XYZ Co"), k("xyz"));
    assert_ne!(k("XYZ Ltd"), k("XYW Ltd"));
}

/// The five groups the Owner asked about on 14-19 Oct, as fixtures.
async fn prod_groups(pool: &PgPool, t: &str) -> Vec<(&'static str, Vec<Uuid>)> {
    let mut out = vec![];
    let names: [(&str, Vec<String>); 5] = [
        (
            "contoso",
            vec![
                format!("Contoso Pacific Partners {t} Ltd"),
                format!("CONTOSO PACIFIC PARTNERS {t} LTD."),
                format!("Contoso Pacific Partners {t}, Inc"),
            ],
        ),
        (
            "fabrikam",
            vec![format!("FabriKam {t} Ltd"), format!("Fabri Kam {t}")],
        ),
        (
            "initrode",
            vec![
                format!("Initrode Kobe {t} Ltd"),
                format!("Initrode - Kobe {t}"),
            ],
        ),
        (
            "xyz",
            vec![format!("XYZ {t} Ltd"), format!("X Y Z {t} Co.")],
        ),
        (
            "umbrella",
            vec![format!("Umbrella {t} Ltd"), format!("umbrella {t}")],
        ),
    ];
    for (key, list) in names {
        let mut ids = vec![];
        for (i, n) in list.iter().enumerate() {
            let phone = if i == 1 { Some("+81811000111") } else { None };
            ids.push(org(pool, n, phone, Some(if i == 0 { "Tokyo" } else { "Osaka" })).await);
        }
        out.push((key, ids));
    }
    out
}

#[tokio::test]
async fn the_five_prod_groups_are_found_with_what_tells_them_apart() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let t = tag();
    let groups = prod_groups(&d.pool, &t).await;
    // a lone company is not a duplicate
    org(&d.pool, &format!("Acme Packaging Ltd {t}"), None, None).await;
    let out = run(
        &d.pool,
        &me,
        "find_duplicate_organizations",
        json!({"query": t, "limit": 50}),
    )
    .await;
    let found = out["groups"].as_array().unwrap_or_else(|| panic!("{out}"));
    assert_eq!(found.len(), 5, "{out}");
    for (key, ids) in &groups {
        let want: HashSet<String> = ids.iter().map(|i| i.to_string()).collect();
        let g = found
            .iter()
            .find(|g| {
                let got: HashSet<String> = g["organizations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| s(&o["id"]).to_string())
                    .collect();
                got == want
            })
            .unwrap_or_else(|| panic!("group {key} not found: {out}"));
        let differs: Vec<&str> = g["what_differs"]
            .as_array()
            .unwrap()
            .iter()
            .map(s)
            .collect();
        assert!(differs.iter().any(|d| d.starts_with("name:")), "{g}");
        assert!(differs.iter().any(|d| d.starts_with("phone:")), "{g}");
        assert!(!g["why_duplicates"].as_array().unwrap().is_empty(), "{g}");
        assert!(want.contains(s(&g["suggested_keep_id"])), "{g}");
    }
    let contoso = run(
        &d.pool,
        &me,
        "find_duplicate_organizations",
        json!({"query": format!("contoso {t}")}),
    )
    .await;
    assert_eq!(contoso["groups"].as_array().unwrap().len(), 1, "{contoso}");
    assert_eq!(
        contoso["groups"][0]["organizations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

/// Rows pointing at an organization, per table (every FK column plus the typed references).
async fn pointing_at(pool: &PgPool, id: Uuid) -> i64 {
    let mut n = 0;
    for (table, fk, _) in dedupe::related("organization") {
        let (c,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM {table} WHERE {fk} = $1"))
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        n += c;
    }
    for (table, tcol, icol, tval, _) in dedupe::related_typed("organization") {
        let (c,): (i64,) = sqlx::query_as(&format!(
            "SELECT count(*) FROM {table} WHERE {tcol} = $2 AND {icol}::text = $1::text"
        ))
        .bind(id)
        .bind(*tval)
        .fetch_one(pool)
        .await
        .unwrap();
        n += c;
    }
    n
}

#[tokio::test]
async fn a_multi_merge_is_one_card_and_nothing_is_lost() {
    let d = db().await;
    let me = person(&d.pool, true, &[]).await;
    let t = tag();
    let groups = prod_groups(&d.pool, &t).await;
    let (keep, b, c) = (groups[0].1[0], groups[0].1[1], groups[0].1[2]);
    let p = &d.pool;
    // linked to B: a contact, a deal, an activity, a file, a project, a sales document, a reminder,
    // Nary's note, research (one person also found for the kept copy, one only here)
    exec(p, "INSERT INTO contacts (id, first_name, organization_id, is_deleted, custom) VALUES (gen_random_uuid(), 'Bob', $1, false, '{}')", &[b]).await;
    exec(p, "INSERT INTO deals (id, title, stage, organization_id, is_deleted, custom) VALUES (gen_random_uuid(), 'Laptops', 'qualifying', $1, false, '{}')", &[b]).await;
    exec(p, "INSERT INTO activities (id, entity_type, entity_id, activity_type, subject) VALUES (gen_random_uuid(), 'organization', $1, 'note', 'visited')", &[b]).await;
    exec(p, "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, file_type) \
             VALUES (gen_random_uuid(), 'crm_org', $1::text, 'tax_id.pdf', 10, 'application/pdf', 'k', 'u', 'document')", &[b]).await;
    exec(p, "INSERT INTO projects (id, name, project_type, customer_org_id, is_deleted) VALUES (gen_random_uuid(), 'Install', 'implementation', $1, false)", &[b]).await;
    exec(p, "INSERT INTO sales_documents (id, number, title, organization_id, is_deleted) VALUES (gen_random_uuid(), $2::text, 'Proposal', $1, false)", &[b, Uuid::new_v4()]).await;
    exec(p, "INSERT INTO nary_memory (id, layer, content, source_type, subject_type, subject_id) VALUES (gen_random_uuid(), 'customer', 'likes WhatsApp', 'owner', 'organization', $1::text)", &[b]).await;
    exec(p, "INSERT INTO crm_research_people (id, organization_id, name, name_key, status) VALUES (gen_random_uuid(), $1, 'Rachel', 'rachel', 'found')", &[keep]).await;
    exec(p, "INSERT INTO crm_research_people (id, organization_id, name, name_key, status) VALUES (gen_random_uuid(), $1, 'Rachel', 'rachel', 'found')", &[b]).await;
    exec(p, "INSERT INTO crm_research_people (id, organization_id, name, name_key, status) VALUES (gen_random_uuid(), $1, 'Diana', 'diana', 'found')", &[b]).await;
    exec(p, "INSERT INTO crm_research_runs (id, object_type, record_id, depth, status) VALUES (gen_random_uuid(), 'organization', $1, 'quick', 'done')", &[b]).await;
    // linked to C: a lead and a quotation; C also has a website the kept copy lacks
    exec(p, "INSERT INTO leads (id, title, source, organization_id, is_deleted, custom) VALUES (gen_random_uuid(), 'Lead', 'website', $1, false, '{}')", &[c]).await;
    exec(p, "INSERT INTO quotations (id, number, issue_date, valid_until, customer_org_id, is_deleted) VALUES (gen_random_uuid(), $2::text, current_date, current_date + 14, $1, false)", &[c, Uuid::new_v4()]).await;
    exec(
        p,
        "UPDATE organizations SET website = 'contoso.example' WHERE id = $1",
        &[c],
    )
    .await;
    exec(p, "INSERT INTO crm_duplicate_candidates (id, object_type, record_a_id, record_b_id, score, reasons, status) \
             VALUES (gen_random_uuid(), 'organization', LEAST($1, $2), GREATEST($1, $2), 90, '[]', 'open')", &[keep, c]).await;
    let before: i64 =
        pointing_at(p, keep).await + pointing_at(p, b).await + pointing_at(p, c).await;

    let out = run(
        p,
        &me,
        "propose_organization_merge",
        json!({"keep_id": keep.to_string(), "merge_ids": [b.to_string(), c.to_string()]}),
    )
    .await;
    let card = &out["proposal"];
    assert_eq!(card["action"], "organization.merge", "{out}");
    let summary = s(&card["summary"]);
    for want in [
        "1 contacts",
        "1 deals",
        "1 activities",
        "1 files",
        "1 projects",
        "1 sales documents",
        "1 Nary memory notes",
        "1 researched people",
        "1 research runs",
        "1 leads",
        "1 quotations",
        "phone",
        "website",
    ] {
        assert!(summary.contains(want), "{want} missing: {summary}");
    }
    assert!(!summary.contains('\u{2014}'));

    // applied as the chat applies it (POST /nary/actions/run)
    let mut conn = p.acquire().await.unwrap();
    let row = executor::run_on_instruction(
        &mut conn,
        &me,
        "organization.merge",
        card["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    let row: granary_db::rows::nary::AgentPendingAction =
        sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
            .bind(row.id)
            .fetch_one(p)
            .await
            .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    let text = cards::render_card(&mut conn, &row).await.unwrap();
    assert!(
        text.contains("Keep: ") && text.contains("Merge away: ") && text.contains("fills phone"),
        "{text}"
    );

    let after_keep = pointing_at(p, keep).await;
    let left_b = pointing_at(p, b).await;
    let left_c = pointing_at(p, c).await;
    assert_eq!(left_c, 0);
    assert_eq!(
        left_b, 1,
        "only the researched person already found for the kept copy stays"
    );
    assert_eq!(after_keep + left_b + left_c, before, "nothing lost");
    let (gone,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM organizations WHERE id = ANY($1) AND is_deleted = true",
    )
    .bind(vec![b, c])
    .fetch_one(p)
    .await
    .unwrap();
    assert_eq!(gone, 2);
    let (phone, website): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT phone, website FROM organizations WHERE id = $1")
            .bind(keep)
            .fetch_one(p)
            .await
            .unwrap();
    assert_eq!(phone.as_deref(), Some("+81811000111"));
    assert_eq!(website.as_deref(), Some("contoso.example"));
    let (open,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM crm_duplicate_candidates WHERE status = 'open' AND (record_a_id = $1 OR record_b_id = $1)",
    )
    .bind(c)
    .fetch_one(p)
    .await
    .unwrap();
    assert_eq!(open, 0);

    // merging a removed copy again is refused with the reason
    let again = run(
        p,
        &me,
        "propose_organization_merge",
        json!({"keep_id": keep.to_string(), "merge_ids": [b.to_string()]}),
    )
    .await;
    assert!(
        s(&again["error"]).contains("already merged or deleted"),
        "{again}"
    );
    let self_merge = run(
        p,
        &me,
        "propose_organization_merge",
        json!({"keep_id": keep.to_string(), "merge_ids": [keep.to_string()]}),
    )
    .await;
    assert!(self_merge.get("error").is_some(), "{self_merge}");
}

// ─── A4: card history ────────────────────────────────────────────────────────

async fn card_row(
    pool: &PgPool,
    action: &str,
    status: &str,
    payload: Value,
    edited: Option<Value>,
    decided_by: Option<Uuid>,
    reason: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, level, customer_facing, payload, edited_payload, status, \
         decided_by_id, decided_at, reject_reason, diff) \
         VALUES ($1, $2, 'nary', 1, true, $3, $4, $5, $6, CASE WHEN $6::uuid IS NULL THEN NULL ELSE now() END, $7, $8)",
    )
    .bind(id)
    .bind(action)
    .bind(&payload)
    .bind(&edited)
    .bind(status)
    .bind(decided_by)
    .bind(reason)
    .bind(json!({"label": format!("{action} card")}))
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn the_card_history_shows_drafts_edits_and_rejections() {
    let d = db().await;
    let p = &d.pool;
    sqlx::query("DELETE FROM agent_pending_actions")
        .execute(p)
        .await
        .unwrap();
    let owner = person(p, true, &[]).await;
    let chat = format!("81901{}@c.us", &tag()[..6]);
    let wa = |text: &str| json!({"channel": "whatsapp", "chat_id": chat, "text": text, "by_owner": false});
    let applied = card_row(
        p,
        "whatsapp.reply.send",
        "applied",
        wa("Good morning Mr Bob, thank you."),
        None,
        Some(owner.id),
        None,
    )
    .await;
    let edited = card_row(
        p,
        "whatsapp.reply.send",
        "applied",
        wa("Good morning Mr Bob, the laptop price is JPY 5 million per unit."),
        Some(wa(
            "Good morning Mr Bob, the laptop price is in the quotation we sent.",
        )),
        Some(owner.id),
        None,
    )
    .await;
    let rejected = card_row(
        p,
        "email.reply.send",
        "rejected",
        json!({"channel": "email", "to": "a@b.test", "text": "Dear Sir"}),
        None,
        Some(owner.id),
        Some("too formal, use Mr"),
    )
    .await;
    card_row(
        p,
        "lead.promote",
        "expired",
        json!({"lead_id": Uuid::new_v4().to_string()}),
        None,
        None,
        None,
    )
    .await;
    card_row(
        p,
        "whatsapp.reply.send",
        "pending",
        wa("Hello"),
        None,
        None,
        None,
    )
    .await;
    card_row(
        p,
        "reengagement.send",
        "cancelled",
        json!({"chat_id": "x@c.us", "text": "How are you?"}),
        None,
        None,
        None,
    )
    .await;

    let all = run(p, &owner, "list_nary_cards", json!({})).await;
    assert_eq!(all["total"], 6, "{all}");
    let ed = run(p, &owner, "list_nary_cards", json!({"status": "edited"})).await;
    assert_eq!(ed["total"], 1, "{ed}");
    let e = &ed["cards"][0];
    assert_eq!(s(&e["id"]), edited.to_string());
    assert_eq!(e["status"], "edited");
    assert!(s(&e["drafted_text"]).contains("JPY 5 million"));
    assert!(s(&e["final_text"]).contains("quotation"));
    let changes = s(&e["changes"]);
    assert!(
        changes.contains("[-") && changes.contains("{+") && changes.contains("quotation"),
        "{changes}"
    );
    assert_eq!(s(&e["decided_by"]), owner.name);

    let rj = run(p, &owner, "list_nary_cards", json!({"status": "rejected"})).await;
    assert_eq!(rj["cards"][0]["id"], rejected.to_string());
    assert_eq!(rj["cards"][0]["reject_reason"], "too formal, use Mr");
    let ap = run(p, &owner, "list_nary_cards", json!({"status": "applied"})).await;
    assert_eq!(ap["total"], 1);
    assert_eq!(ap["cards"][0]["id"], applied.to_string());
    assert!(ap["cards"][0].get("final_text").is_none());
    for (status, n) in [("expired", 1), ("cancelled", 1), ("pending", 1)] {
        let out = run(p, &owner, "list_nary_cards", json!({"status": status})).await;
        assert_eq!(out["total"], n, "{status}: {out}");
    }
    // filters: chat, kind family, paging
    let by_chat = run(p, &owner, "list_nary_cards", json!({"chat_id": chat})).await;
    assert_eq!(by_chat["total"], 3, "{by_chat}");
    let fam = run(p, &owner, "list_nary_cards", json!({"kind": "whatsapp"})).await;
    assert_eq!(fam["total"], 3, "{fam}");
    let page1 = run(p, &owner, "list_nary_cards", json!({"limit": 4})).await;
    let page2 = run(
        p,
        &owner,
        "list_nary_cards",
        json!({"limit": 4, "offset": 4}),
    )
    .await;
    assert_eq!(
        (page1["count"].as_i64(), page2["count"].as_i64()),
        (Some(4), Some(2))
    );
    assert_eq!(page2["total"], 6);
    assert!(
        run(p, &owner, "list_nary_cards", json!({"status": "bogus"}))
            .await
            .get("error")
            .is_some()
    );
    // a Nary admin may read it; a salesperson may not
    let admin = person(p, false, &["nary:manage"]).await;
    assert_eq!(
        run(p, &admin, "list_nary_cards", json!({})).await["total"],
        6
    );
    let sales = person(p, false, &["crm:view_all", "crm:edit_contact"]).await;
    let denied = run(p, &sales, "list_nary_cards", json!({})).await;
    assert!(s(&denied["error"]).contains("permission"), "{denied}");
}

#[test]
fn the_short_diff_marks_only_what_changed() {
    let d = hygiene::short_diff(
        "one two three four five six seven eight nine ten",
        "one two three four five SIX seven eight nine ten",
    );
    assert_eq!(
        d,
        "... three four five [-six-] {+SIX+} seven eight nine ..."
    );
    assert_eq!(hygiene::short_diff("same words", "same  words"), "");
}

// ─── A9: delete a contact ────────────────────────────────────────────────────

#[tokio::test]
async fn only_the_owner_deletes_a_contact_and_open_deals_must_be_on_the_card() {
    let d = db().await;
    let p = &d.pool;
    let owner = person(p, true, &[]).await;
    let me_as_customer = Uuid::new_v4();
    exec(p, "INSERT INTO contacts (id, first_name, last_name, phone, is_deleted, custom) VALUES ($1, 'Smith', 'Smith', '+81811999', false, '{}')", &[me_as_customer]).await;
    let deal = Uuid::new_v4();
    exec(p, "INSERT INTO deals (id, title, stage, contact_id, is_deleted, custom) VALUES ($1, 'Test deal', 'qualifying', $2, false, '{}')", &[deal, me_as_customer]).await;

    // not the Owner: refused even with every CRM permission
    let staff = person(
        p,
        false,
        &["nary:approve", "crm:delete_contact", "crm:view_all"],
    )
    .await;
    let no = run(
        p,
        &staff,
        "propose_contact_delete",
        json!({"contact_id": me_as_customer.to_string()}),
    )
    .await;
    assert!(s(&no["error"]).contains("Only the Owner"), "{no}");

    // open deal: refused with the deal named, then a card that lists it
    let held = run(
        p,
        &owner,
        "propose_contact_delete",
        json!({"contact_id": me_as_customer.to_string()}),
    )
    .await;
    assert!(
        s(&held["error"]).contains("Test deal") && s(&held["error"]).contains("include_open"),
        "{held}"
    );
    let out = run(
        p,
        &owner,
        "propose_contact_delete",
        json!({"contact_id": me_as_customer.to_string(), "include_open": true, "reason": "our own number"}),
    )
    .await;
    let card = &out["proposal"];
    assert_eq!(card["action"], "contact.delete", "{out}");
    assert!(
        s(&card["summary"]).contains("Test deal") && s(&card["summary"]).contains("our own number")
    );
    assert_eq!(card["payload"]["open_records"], json!([deal.to_string()]));

    // a deal opened after the card was made stops it
    let late = Uuid::new_v4();
    exec(p, "INSERT INTO deals (id, title, stage, contact_id, is_deleted, custom) VALUES ($1, 'Late deal', 'qualifying', $2, false, '{}')", &[late, me_as_customer]).await;
    let mut conn = p.acquire().await.unwrap();
    let stopped = executor::run_on_instruction(
        &mut conn,
        &owner,
        "contact.delete",
        card["payload"].clone(),
        None,
    )
    .await;
    assert!(stopped.is_err());
    let (deleted,): (bool,) = sqlx::query_as("SELECT is_deleted FROM contacts WHERE id = $1")
        .bind(me_as_customer)
        .fetch_one(p)
        .await
        .unwrap();
    assert!(!deleted);
    exec(p, "UPDATE deals SET stage = 'lost' WHERE id = $1", &[late]).await;

    let row = executor::run_on_instruction(
        &mut conn,
        &owner,
        "contact.delete",
        card["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    let row: granary_db::rows::nary::AgentPendingAction =
        sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
            .bind(row.id)
            .fetch_one(p)
            .await
            .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    let text = cards::render_card(&mut conn, &row).await.unwrap();
    assert!(
        text.contains("Test deal") && text.contains("Reason: our own number"),
        "{text}"
    );
    let (deleted, at): (bool, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT is_deleted, deleted_at FROM contacts WHERE id = $1")
            .bind(me_as_customer)
            .fetch_one(p)
            .await
            .unwrap();
    assert!(deleted && at.is_some());
    let (audit,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM audit_logs WHERE action = 'delete' AND entity_type = 'contact' AND entity_id = $1",
    )
    .bind(me_as_customer.to_string())
    .fetch_one(p)
    .await
    .unwrap();
    assert_eq!(audit, 1);
}

// ─── the tools themselves ────────────────────────────────────────────────────

#[test]
fn the_new_tools_are_listed_with_valid_schemas_and_mcp_levels() {
    use granary_services::ai::mcp_access::PROPOSAL_TOOLS;
    for name in [
        "find_duplicate_organizations",
        "propose_organization_merge",
        "list_nary_cards",
        "propose_contact_delete",
    ] {
        let tool = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} not in the catalog"));
        // as written: no list of types next to an enum (the 15 Oct 400); as sent: the Anthropic rules
        assert!(
            !schema::violations(&tool.parameters)
                .iter()
                .any(|v| v.contains("an enum with a list of types")),
            "{name}"
        );
        assert!(
            schema::violations(&schema::for_anthropic(&tool.parameters)).is_empty(),
            "{name}"
        );
        assert!(!tool.description.contains('\u{2014}'), "{name}");
        assert_eq!(
            PROPOSAL_TOOLS.contains(&name),
            name.starts_with("propose_"),
            "{name}: propose tools are change tools, the others read"
        );
    }
    for a in ["organization.merge", "contact.delete"] {
        assert!(executor::CHAT_ACTIONS.contains(&a), "{a}");
        assert!(granary_services::nary::actions::get(a).is_ok(), "{a}");
    }
}

/// The baseline defaults (were migration 0110): both actions ask, and are never promoted past asking.
#[tokio::test]
async fn merge_and_delete_ask_and_are_never_promoted_by_default() {
    let d = db().await;
    let levels: (String,) =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
            .fetch_one(&d.pool)
            .await
            .unwrap();
    let levels: Value = serde_json::from_str(&levels.0).unwrap();
    assert_eq!(levels["organization.merge"], 1);
    assert_eq!(levels["contact.delete"], 1);
    assert_eq!(levels["contact.merge"], 1, "existing keys kept");
    let never: (String,) = sqlx::query_as(
        "SELECT value FROM system_settings WHERE key = 'nary.autonomy.never_promote'",
    )
    .fetch_one(&d.pool)
    .await
    .unwrap();
    let never: Vec<String> = serde_json::from_str(&never.0).unwrap();
    assert!(
        never.contains(&"organization.merge".to_string())
            && never.contains(&"contact.delete".to_string())
    );
    assert!(
        never.contains(&"quotation.create_draft".to_string()),
        "existing entries kept"
    );
}
