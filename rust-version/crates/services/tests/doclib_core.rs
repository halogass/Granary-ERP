//! Document library, agent A1 (21 Oct 2026): the backend core.
//!
//! Create, the label suggestion table, internal versions live at once, customer versions pending -> approve /
//! reject (and a pending version replaced by a newer upload, the Owner's own upload approved on the spot),
//! owner vs manager vs executive rights, history hidden from non-admins, restore, category role visibility,
//! search filters and scopes, the review reminder, the product files migration (idempotent), upload checks.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use granary_core::auth::{load_active_user, query_permissions, AuthUser};
use granary_services::approvals::engine;
use granary_services::doc_library::versions::{self, NewDocument, NewFile, NewVersion, ProductRef};
use granary_services::doc_library::{
    self as lib, access, migrate_product_files, notify, view, Filters, Scope,
};
use granary_services::pdftools::{Document as Pdf, Point, SaveOptions, TextOptions};
use granary_services::storage::{self, ObjectStore, StoreError};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

// ─── fakes and builders ─────────────────────────────────────────────────────

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ObjectStore for MemStore {
    async fn bucket_exists(&self, _b: &str) -> Result<bool, StoreError> {
        Ok(true)
    }
    async fn make_bucket(&self, _b: &str) -> Result<(), StoreError> {
        Ok(())
    }
    async fn put_object(
        &self,
        _b: &str,
        key: &str,
        data: &[u8],
        _ct: &str,
    ) -> Result<(), StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .insert(key.to_string(), data.to_vec());
        Ok(())
    }
    async fn get_object(&self, _b: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(StoreError {
                kind: "S3Error".into(),
                message: key.into(),
            })
    }
}

fn pdf(text: &str) -> Vec<u8> {
    let mut doc = Pdf::new().unwrap();
    let p = doc.new_page(595.0, 842.0).unwrap();
    doc.insert_text(p, Point::new(50.0, 80.0), text, &TextOptions::default())
        .unwrap();
    doc.tobytes(SaveOptions::default()).unwrap()
}

fn file(name: &str, data: Vec<u8>) -> NewFile {
    let mime = if name.ends_with(".pdf") {
        Some("application/pdf".to_string())
    } else {
        None
    };
    NewFile {
        data,
        file_name: name.into(),
        mime,
    }
}

async fn role_id(db: &mut PgConnection, slug: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE slug = $1")
        .bind(slug)
        .fetch_one(&mut *db)
        .await
        .unwrap_or_else(|_| panic!("role {slug}"))
}

async fn person(pool: &PgPool, name: &str, roles: &[&str], superadmin: bool) -> AuthUser {
    let mut c = pool.acquire().await.unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password) VALUES ($1, $2, $3, 'x', true, $4, false, 'internal', true, false)",
    )
    .bind(id)
    .bind(format!("{}.{}@granary.test", name.to_lowercase(), &id.to_string()[..6]))
    .bind(name)
    .bind(superadmin)
    .execute(&mut *c)
    .await
    .unwrap();
    for slug in roles {
        let rid = role_id(&mut c, slug).await;
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(id)
            .bind(rid)
            .execute(&mut *c)
            .await
            .unwrap();
    }
    let mut u = load_active_user(&mut c, id).await.unwrap().unwrap();
    u.perms = query_permissions(&mut c, id).await.unwrap();
    u
}

async fn product(pool: &PgPool, sku: &str, family: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, family, product_type, unit, is_active, sale_model, price_metric, \
         modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) \
         VALUES ($1, $2, $3, $4, 'physical', 'Unit', true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
    )
    .bind(id)
    .bind(sku)
    .bind(format!("{sku} product"))
    .bind(family)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn meta(title: &str, category: &str, audiences: &[&str]) -> NewDocument {
    NewDocument {
        title: title.into(),
        category: category.into(),
        audiences: audiences.iter().map(|s| s.to_string()).collect(),
        ..NewDocument::default()
    }
}

fn version(note: &str, text: &str) -> NewVersion {
    NewVersion {
        label: None,
        change_note: note.into(),
        file: file("doc.pdf", pdf(text)),
        source: None,
    }
}

async fn bells(db: &mut PgConnection, user: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND type = 'document_library'",
    )
    .bind(user)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

async fn status_of(db: &mut PgConnection, vid: Uuid) -> String {
    lib::load_version(db, vid).await.unwrap().unwrap().status
}

fn staff(user: &AuthUser) -> Scope<'_> {
    Scope::Staff(user)
}

async fn titles(
    db: &mut PgConnection,
    scope: &Scope<'_>,
    q: Option<&str>,
    f: Filters,
) -> Vec<String> {
    let mut t: Vec<String> = lib::search(db, scope, q, &f)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|d| d.title)
        .collect();
    t.sort();
    t
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[test]
fn label_suggestions() {
    for (prev, next) in [
        (None, "v1"),
        (Some("v33"), "v34"),
        (Some("v2.1"), "v2.2"),
        (Some("3"), "4"),
        (Some("Rev B2"), "Rev B3"),
        (Some("final"), "final 2"),
        (Some("v9"), "v10"),
        (Some("v009"), "v010"),
        (Some(" v4 "), "v5"),
        (Some(""), "v1"),
    ] {
        assert_eq!(lib::suggest_label(prev), next, "{prev:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn library_core() {
    let pool = support::fresh_db("doclib_core").await;
    let store = Arc::new(MemStore::default());
    storage::set_store(Some(store.clone()));
    let mut c = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut c;

    let owner = person(&pool, "Owner", &[], true).await;
    let exec = person(&pool, "Exec", &["executive"], false).await;
    let exec2 = person(&pool, "Exec2", &["executive"], false).await;
    let marketer = person(&pool, "Marketer", &["marketing"], false).await;
    let marketer2 = person(&pool, "Marketer2", &["marketing"], false).await;
    let engineer = person(&pool, "Engineer", &["engineer"], false).await;
    let sysadmin = person(&pool, "Sysadmin", &["system_admin"], false).await;
    let partner = {
        let mut p = person(&pool, "Partner", &["partner_portal"], false).await;
        p.portal_type = "partner".into();
        p
    };

    // ── rights in the abstract
    assert!(access::can_create(&marketer));
    assert!(!access::can_create(&engineer));
    assert!(access::is_manager_all(&exec) && access::can_approve(&exec));
    assert!(access::is_doc_admin(&sysadmin) && !access::can_approve(&sysadmin));
    assert!(!access::is_doc_admin(&marketer));
    assert!(!access::can_view_category(db, &partner, Uuid::nil())
        .await
        .unwrap());

    // ── create an internal document: live at once, staff hear about it (not the uploader)
    let before_eng = bells(db, engineer.id).await;
    let (internal, v1) = versions::create_document(
        db,
        &marketer,
        NewDocument {
            review_by: Some(chrono::Local::now().date_naive() + chrono::Duration::days(3)),
            ..meta("Company profile", "company", &["internal"])
        },
        file("profile.pdf", pdf("ACME company profile body")),
        None,
    )
    .await
    .unwrap();
    assert_eq!(v1.label, "v1");
    assert_eq!(v1.status, "live");
    assert_eq!(v1.change_note, "First version");
    assert_eq!(internal.current_version_id, Some(v1.id));
    assert_eq!(internal.owner_id, Some(marketer.id));
    assert!(v1
        .extracted_units
        .as_ref()
        .unwrap()
        .to_string()
        .contains("company profile body"));
    assert_eq!(v1.page_count, Some(1));
    assert_eq!(bells(db, engineer.id).await, before_eng + 1);
    assert_eq!(bells(db, marketer.id).await, 0);
    // stored in the private bucket, content addressed
    assert!(store
        .blobs
        .lock()
        .unwrap()
        .contains_key(&v1.file_object_key));
    assert!(v1.file_object_key.starts_with("doclib/"));

    // engineer cannot create
    let e = versions::create_document(
        db,
        &engineer,
        meta("X", "company", &["internal"]),
        file("x.pdf", pdf("x")),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 403);

    // ── a second internal version: suggested label, previous superseded
    assert_eq!(lib::next_label(db, internal.id).await.unwrap(), "v2");
    let v2 = versions::add_version(
        db,
        &marketer,
        &internal,
        version("New team page", "profile v2"),
    )
    .await
    .unwrap();
    assert_eq!((v2.label.as_str(), v2.status.as_str()), ("v2", "live"));
    assert_eq!(status_of(db, v1.id).await, "superseded");
    assert_eq!(
        lib::current_live(db, internal.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        v2.id
    );

    // ── owner vs other creator vs director
    let e = versions::add_version(db, &marketer2, &internal, version("not mine", "x"))
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 403);
    let e = versions::add_version(db, &engineer, &internal, version("no", "x"))
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 403);
    let v3 = versions::add_version(
        db,
        &exec2,
        &internal,
        NewVersion {
            label: Some("2026 edition".into()),
            ..version("Director fix", "profile v3")
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (v3.label.as_str(), v3.status.as_str()),
        ("2026 edition", "live")
    );
    assert_eq!(
        lib::next_label(db, internal.id).await.unwrap(),
        "2027 edition"
    );
    // a label already taken
    let e = versions::add_version(
        db,
        &marketer,
        &internal,
        NewVersion {
            label: Some("v2".into()),
            ..version("dup", "x")
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 409);
    // change note required
    let e = versions::add_version(db, &marketer, &internal, version("  ", "x"))
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 400);

    // ── a customer document: pending until a director approves
    let p1 = product(&pool, "LAPTOP-TM", "laptop").await;
    let p2 = product(&pool, "LAPTOP-FR", "laptop").await;
    let p3 = product(&pool, "MON-1", "monitor").await;
    let (brochure, b1) = versions::create_document(
        db,
        &marketer,
        NewDocument {
            products: vec![ProductRef {
                product_id: Some(p1),
                family: None,
            }],
            ..meta("Laptop brochure", "products", &["customer"])
        },
        file("brochure.pdf", pdf("Laptop office bundle brochure")),
        None,
    )
    .await
    .unwrap();
    assert_eq!(b1.status, "pending_approval");
    assert!(lib::current_live(db, brochure.id).await.unwrap().is_none());
    let req = engine::latest_request(db, versions::ENTITY_TYPE, b1.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(req.status, "pending");
    // pending view
    let pend = titles(
        db,
        &staff(&engineer),
        None,
        Filters {
            view: Some("pending".into()),
            ..Filters::default()
        },
    )
    .await;
    assert_eq!(pend, vec!["Laptop brochure"]);
    // a non-director cannot approve
    assert!(engine::act_on_pending(
        db,
        versions::ENTITY_TYPE,
        b1.id,
        &marketer2,
        "approved",
        None
    )
    .await
    .is_err());
    engine::act_on_pending(db, versions::ENTITY_TYPE, b1.id, &exec, "approved", None)
        .await
        .unwrap();
    let b1 = lib::load_version(db, b1.id).await.unwrap().unwrap();
    assert_eq!(b1.status, "live");
    assert_eq!(b1.approved_by, Some(exec.id));
    assert!(b1.approved_at.is_some() && b1.went_live_at.is_some());
    assert_eq!(
        lib::current_live(db, brochure.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        b1.id
    );

    // reject: stays on v1, the reason kept
    let b2 = versions::add_version(db, &marketer, &brochure, version("Typo fix", "brochure v2"))
        .await
        .unwrap();
    assert_eq!(b2.status, "pending_approval");
    engine::act_on_pending(
        db,
        versions::ENTITY_TYPE,
        b2.id,
        &exec,
        "rejected",
        Some("Wrong logo"),
    )
    .await
    .unwrap();
    let b2 = lib::load_version(db, b2.id).await.unwrap().unwrap();
    assert_eq!(b2.status, "rejected");
    assert_eq!(b2.rejected_reason.as_deref(), Some("Wrong logo"));
    assert_eq!(
        lib::current_live(db, brochure.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        b1.id
    );

    // a pending version replaced by a newer upload
    let b3 = versions::add_version(
        db,
        &marketer,
        &brochure,
        version("Price table out", "brochure v3"),
    )
    .await
    .unwrap();
    assert_eq!(b3.label, "v3");
    let b4 = versions::add_version(
        db,
        &marketer,
        &brochure,
        version("Price table out, really", "brochure v4"),
    )
    .await
    .unwrap();
    let b3 = lib::load_version(db, b3.id).await.unwrap().unwrap();
    assert_eq!(b3.status, "rejected");
    assert_eq!(
        b3.rejected_reason.as_deref(),
        Some("Replaced by v4 before approval")
    );
    assert_eq!(b4.status, "pending_approval");
    // the director who submitted cannot approve it himself; another director can
    let b5 = versions::add_version(db, &exec, &brochure, version("Exec edit", "brochure v5"))
        .await
        .unwrap();
    assert_eq!(b5.status, "pending_approval");
    assert!(
        engine::act_on_pending(db, versions::ENTITY_TYPE, b5.id, &exec, "approved", None)
            .await
            .is_err()
    );
    engine::act_on_pending(db, versions::ENTITY_TYPE, b5.id, &exec2, "approved", None)
        .await
        .unwrap();
    assert_eq!(status_of(db, b5.id).await, "live");
    assert_eq!(status_of(db, b1.id).await, "superseded");
    // the Owner's own upload is approved on the spot
    let b6 = versions::add_version(db, &owner, &brochure, version("Owner edit", "brochure v6"))
        .await
        .unwrap();
    assert_eq!(b6.status, "live");
    assert_eq!(b6.approved_by, Some(owner.id));

    // ── history hidden from non-admins; permissions
    let d_eng = view::detail(db, &engineer, &brochure).await.unwrap();
    assert!(d_eng["versions"].is_null());
    assert_eq!(d_eng["permissions"]["approve"], false);
    assert_eq!(d_eng["permissions"]["edit"], false);
    assert_eq!(d_eng["current_version"]["label"], "v6");
    let d_owner = view::detail(db, &marketer, &brochure).await.unwrap();
    assert!(d_owner["versions"].is_null());
    assert_eq!(d_owner["permissions"]["edit"], true);
    assert_eq!(d_owner["permissions"]["history"], false);
    let d_sys = view::detail(db, &sysadmin, &brochure).await.unwrap();
    assert_eq!(d_sys["versions"].as_array().unwrap().len(), 6);
    assert_eq!(d_sys["permissions"]["approve"], false);
    assert_eq!(d_sys["permissions"]["trace"], true);
    let d_exec = view::detail(db, &exec, &brochure).await.unwrap();
    assert_eq!(d_exec["permissions"]["approve"], true);
    assert_eq!(d_exec["products"][0]["sku"], "LAPTOP-TM");

    // ── restore: doc admins only, a new version with the old files, same approval rules
    let e = versions::restore(db, &marketer, &internal, v1.id)
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 403);
    let r = versions::restore(db, &sysadmin, &internal, v1.id)
        .await
        .unwrap();
    assert_eq!(r.label, "2027 edition");
    assert_eq!(r.change_note, "Restored from v1");
    assert_eq!(r.status, "live");
    assert_eq!(r.file_sha256, v1.file_sha256);
    assert_eq!(r.file_object_key, v1.file_object_key);
    let rb = versions::restore(db, &sysadmin, &brochure, b1.id)
        .await
        .unwrap();
    assert_eq!(rb.status, "pending_approval");
    engine::act_on_pending(db, versions::ENTITY_TYPE, rb.id, &exec, "approved", None)
        .await
        .unwrap();

    // ── category visibility by role
    sqlx::query("UPDATE doc_categories SET visible_roles = '{executive}' WHERE key = 'pricing'")
        .execute(&mut *db)
        .await
        .unwrap();
    let (secret, _) = versions::create_document(
        db,
        &exec,
        meta("Margin sheet", "pricing", &["internal"]),
        file("m.pdf", pdf("margins")),
        None,
    )
    .await
    .unwrap();
    assert!(!access::can_view(db, &engineer, &secret).await.unwrap());
    assert!(access::can_view(db, &exec2, &secret).await.unwrap());
    assert!(access::can_view(db, &sysadmin, &secret).await.unwrap());
    assert_eq!(
        access::viewable(db, &marketer, secret.id)
            .await
            .unwrap_err()
            .status()
            .as_u16(),
        404
    );
    let all_eng = titles(db, &staff(&engineer), None, Filters::default()).await;
    assert!(!all_eng.contains(&"Margin sheet".to_string()));
    assert!(titles(db, &staff(&exec2), None, Filters::default())
        .await
        .contains(&"Margin sheet".to_string()));
    // a marketer cannot create in a category they cannot see
    let e = versions::create_document(
        db,
        &marketer,
        meta("P", "pricing", &["internal"]),
        file("p.pdf", pdf("p")),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 403);
    // the notice of the new pricing version reached only those who can see it
    let eng_bells = bells(db, engineer.id).await;
    let exec_bells_before = bells(db, exec.id).await;
    versions::add_version(db, &exec2, &secret, version("Q4 margins", "m2"))
        .await
        .unwrap();
    assert_eq!(bells(db, engineer.id).await, eng_bells);
    assert_eq!(bells(db, exec.id).await, exec_bells_before + 1);

    // ── search filters
    let (id_doc, _) = versions::create_document(
        db,
        &marketer,
        NewDocument {
            language: Some("fr".into()),
            variant_of: Some(brochure.id),
            products: vec![ProductRef {
                product_id: None,
                family: Some("laptop".into()),
            }],
            ..meta("Laptop brochure FR", "products", &["customer", "partner"])
        },
        file("brochure_fr.pdf", pdf("Brochure ordinateur portable")),
        None,
    )
    .await
    .unwrap();
    let (_cam, _) = versions::create_document(
        db,
        &owner,
        NewDocument {
            products: vec![ProductRef {
                product_id: Some(p3),
                family: None,
            }],
            ..meta("Monitor datasheet", "technical", &["customer"])
        },
        file("monitor.pdf", pdf("Monitor datasheet 27 inch panel")),
        None,
    )
    .await
    .unwrap();
    let f = |view: &str| Filters {
        view: Some(view.into()),
        ..Filters::default()
    };
    let owner_scope = staff(&owner);
    // q matches title and the live text
    assert_eq!(
        titles(db, &owner_scope, Some("inch panel"), Filters::default()).await,
        vec!["Monitor datasheet"]
    );
    assert_eq!(
        titles(db, &owner_scope, Some("brochure"), Filters::default()).await,
        vec!["Laptop brochure", "Laptop brochure FR"]
    );
    // category, audience, language, product (direct and by family), mine
    assert_eq!(
        titles(
            db,
            &owner_scope,
            None,
            Filters {
                category: Some("technical".into()),
                ..Filters::default()
            }
        )
        .await,
        vec!["Monitor datasheet"]
    );
    assert_eq!(
        titles(
            db,
            &owner_scope,
            None,
            Filters {
                language: Some("fr".into()),
                ..Filters::default()
            }
        )
        .await,
        vec!["Laptop brochure FR"]
    );
    assert_eq!(
        titles(
            db,
            &owner_scope,
            None,
            Filters {
                audience: Some("partner".into()),
                ..Filters::default()
            }
        )
        .await,
        vec!["Laptop brochure FR"]
    );
    assert_eq!(
        titles(
            db,
            &owner_scope,
            None,
            Filters {
                product_id: Some(p1),
                ..Filters::default()
            }
        )
        .await,
        vec!["Laptop brochure", "Laptop brochure FR"]
    );
    assert_eq!(
        titles(
            db,
            &owner_scope,
            None,
            Filters {
                product_id: Some(p2),
                ..Filters::default()
            }
        )
        .await,
        vec!["Laptop brochure FR"]
    );
    assert_eq!(
        titles(db, &staff(&marketer), None, f("mine")).await,
        vec!["Company profile", "Laptop brochure", "Laptop brochure FR"]
    );
    assert_eq!(
        titles(db, &owner_scope, None, f("internal")).await,
        vec!["Company profile", "Margin sheet"]
    );
    assert_eq!(
        titles(db, &owner_scope, None, f("customer")).await,
        vec!["Laptop brochure", "Laptop brochure FR", "Monitor datasheet"]
    );
    // paging and total
    let page = lib::search(
        db,
        &owner_scope,
        None,
        &Filters {
            limit: Some(2),
            ..Filters::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((page.items.len(), page.total), (2, 5));
    // a customer scope: only live customer documents (the French brochure is pending: the marketer uploaded it)
    let cust = titles(
        db,
        &Scope::Audiences(vec!["customer".into()]),
        None,
        Filters::default(),
    )
    .await;
    assert_eq!(cust, vec!["Laptop brochure", "Monitor datasheet"]);
    let partner_docs = titles(
        db,
        &Scope::Audiences(vec!["partner".into()]),
        None,
        Filters::default(),
    )
    .await;
    assert!(partner_docs.is_empty());
    // language variants
    let brochure_now = lib::load_document(db, brochure.id).await.unwrap().unwrap();
    assert!(brochure_now.language_group_id.is_some());
    assert_eq!(
        brochure_now.language_group_id,
        lib::load_document(db, id_doc.id)
            .await
            .unwrap()
            .unwrap()
            .language_group_id
    );
    let item = view::item(db, Some(&owner), &brochure_now).await.unwrap();
    assert_eq!(
        item["language_variants"][0]["id"],
        serde_json::json!(id_doc.id)
    );
    assert_eq!(item["language_variants"][0]["language"], "fr");
    // the price list is only listed where asked for
    sqlx::query("UPDATE library_documents SET kind = 'price_list' WHERE id = $1")
        .bind(secret.id)
        .execute(&mut *db)
        .await
        .unwrap();
    assert!(!titles(db, &owner_scope, None, Filters::default())
        .await
        .contains(&"Margin sheet".to_string()));
    assert!(titles(
        db,
        &owner_scope,
        None,
        Filters {
            include_price_list: true,
            ..Filters::default()
        }
    )
    .await
    .contains(&"Margin sheet".to_string()));

    // ── review reminders: due soon, overdue, not again within 7 days
    sqlx::query("UPDATE library_documents SET review_by = CURRENT_DATE - 2 WHERE id = $1")
        .bind(brochure.id)
        .execute(&mut *db)
        .await
        .unwrap();
    let marketer_before = bells(db, marketer.id).await;
    assert_eq!(notify::review_reminders(db).await.unwrap(), 2);
    assert_eq!(bells(db, marketer.id).await, marketer_before + 2);
    let titles_now: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM notifications WHERE user_id = $1 AND type = 'document_library' AND title LIKE 'Review%' ORDER BY title",
    )
    .bind(marketer.id)
    .fetch_all(&mut *db)
    .await
    .unwrap();
    assert_eq!(
        titles_now,
        vec![
            "Review due: Company profile",
            "Review overdue: Laptop brochure"
        ]
    );
    assert_eq!(notify::review_reminders(db).await.unwrap(), 0);
    assert_eq!(
        titles(db, &owner_scope, None, f("needs_review")).await,
        vec!["Laptop brochure"]
    );
    sqlx::query(
        "UPDATE library_documents SET last_reminded_at = now() - interval '8 days' WHERE id = $1",
    )
    .bind(brochure.id)
    .execute(&mut *db)
    .await
    .unwrap();
    assert_eq!(notify::review_reminders(db).await.unwrap(), 1);

    // ── upload checks
    let e = versions::add_version(
        db,
        &marketer,
        &internal,
        NewVersion {
            file: file("profile.docx", b"PK\x03\x04 not really".to_vec()),
            ..version("docx", "x")
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 400);
    let e = versions::add_version(
        db,
        &marketer,
        &internal,
        NewVersion {
            file: file("notes.txt", b"hello".to_vec()),
            ..version("txt", "x")
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 415);
    let with_source = versions::add_version(
        db,
        &marketer,
        &internal,
        NewVersion {
            source: Some(file("profile.docx", b"PK\x03\x04 editable".to_vec())),
            ..version("With source", "profile with source")
        },
    )
    .await
    .unwrap();
    assert!(with_source
        .source_object_key
        .as_deref()
        .unwrap()
        .starts_with("doclib/src/"));
    assert_eq!(
        with_source.source_file_name.as_deref(),
        Some("profile.docx")
    );
    sqlx::query("UPDATE system_settings SET value = '1' WHERE key = 'documents.max_file_mb'")
        .execute(&mut *db)
        .await
        .unwrap();
    let big = {
        let mut b = pdf("big");
        b.extend(vec![b' '; 1_100_000]);
        b
    };
    let e = versions::add_version(
        db,
        &marketer,
        &internal,
        NewVersion {
            file: file("big.pdf", big),
            ..version("big", "x")
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 413);
    sqlx::query("UPDATE system_settings SET value = '100' WHERE key = 'documents.max_file_mb'")
        .execute(&mut *db)
        .await
        .unwrap();

    // ── catalog product files move into the library, once
    let att_pdf = "attachments/catalog_product/datasheet.pdf";
    let att_docx = "attachments/catalog_product/specs.docx";
    store
        .blobs
        .lock()
        .unwrap()
        .insert(att_pdf.into(), pdf("Office laptop datasheet 16 GB RAM"));
    store
        .blobs
        .lock()
        .unwrap()
        .insert(att_docx.into(), b"PK\x03\x04".to_vec());
    for (key, name, mime) in [
        (att_pdf, "TM datasheet.pdf", "application/pdf"),
        (
            att_docx,
            "specs.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
    ] {
        sqlx::query(
            "INSERT INTO attachments (id, entity_type, entity_id, uploaded_by_id, file_name, file_size, mime_type, \
             object_key, url, file_type) VALUES ($1, 'catalog_product', $2, $3, $4, 10, $5, $6, '', 'file')",
        )
        .bind(Uuid::new_v4())
        .bind(p1.to_string())
        .bind(exec.id)
        .bind(name)
        .bind(mime)
        .bind(key)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    let r = migrate_product_files::run(db).await.unwrap();
    assert_eq!((r.migrated, r.skipped_type, r.failed), (1, 1, 0));
    let moved = titles(db, &owner_scope, Some("TM datasheet"), Filters::default()).await;
    assert_eq!(moved, vec!["TM datasheet"]);
    let doc_id: Uuid =
        sqlx::query_scalar("SELECT id FROM library_documents WHERE title = 'TM datasheet'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let d = lib::load_document(db, doc_id).await.unwrap().unwrap();
    assert_eq!(d.audiences, vec!["customer"]);
    assert_eq!(d.owner_id, Some(exec.id));
    let cat: String = sqlx::query_scalar("SELECT key FROM doc_categories WHERE id = $1")
        .bind(d.category_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(cat, "products");
    let live = lib::current_live(db, doc_id).await.unwrap().unwrap();
    assert_eq!(
        (live.label.as_str(), live.change_note.as_str()),
        ("v1", migrate_product_files::NOTE)
    );
    assert_eq!(live.file_object_key, att_pdf);
    assert!(live
        .extracted_units
        .unwrap()
        .to_string()
        .contains("16 GB RAM"));
    assert_eq!(
        titles(
            db,
            &owner_scope,
            None,
            Filters {
                product_id: Some(p1),
                ..Filters::default()
            }
        )
        .await,
        vec!["Laptop brochure", "Laptop brochure FR", "TM datasheet"]
    );
    assert_eq!(
        migrate_product_files::migrated_keys(db, &p1.to_string())
            .await
            .unwrap(),
        vec![att_pdf.to_string()]
    );
    // the flag stops a second run; without the flag nothing is duplicated
    assert!(migrate_product_files::run(db).await.unwrap().already_done);
    sqlx::query("UPDATE system_settings SET value = 'false' WHERE key = $1")
        .bind(migrate_product_files::FLAG)
        .execute(&mut *db)
        .await
        .unwrap();
    let again = migrate_product_files::run(db).await.unwrap();
    assert_eq!((again.migrated, again.skipped_existing), (0, 1));
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM library_documents WHERE title = 'TM datasheet'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(n, 1);
    // the attachment rows stay
    let atts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attachments WHERE entity_type = 'catalog_product'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(atts, 2);
}
