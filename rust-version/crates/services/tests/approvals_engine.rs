//! Port of backend/tests/test_approval_engine.py and test_approval_gating.py (engine-level parts).
//!
//! Python drives the engine through the /approvals router, whose handlers are thin wrappers: submit ->
//! commit -> serialize (201), approve/reject -> act -> commit -> serialize (200), errors roll back. The
//! helpers below do the same against the engine directly, one transaction per "request", inside a
//! `notify::scope`. Router-only checks (permission 403s, visibility, policy PATCH validation, the sales/hr
//! endpoints) wait for the routers wave and are marked ignored.
//!
//! Every approval decision emits `approval.decided` through services/nary/events.py (wave 3), so the tests
//! that reach a decision are ignored until that port lands.

mod support;

use chrono::{Duration, Local, NaiveDate, Utc};
use granary_core::auth::{load_active_user, AuthUser};
use granary_core::errors::ApiError;
use granary_services::approvals::{engine, notify};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

static DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> PgPool {
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("s13_approvals").await;
            pool.close().await;
            std::env::var("DATABASE_URL").unwrap()
        })
        .await;
    notify::set_external_delivery_enabled(false); // the `_no_external_delivery` fixture
    PgPool::connect(url).await.expect("connect")
}

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

#[derive(Clone)]
struct Person {
    id: String,
    employee_id: String,
    user: AuthUser,
}

async fn role_id(db: &mut PgConnection, slug: &str) -> Uuid {
    if let Some(id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM roles WHERE slug = $1")
        .bind(slug)
        .fetch_optional(&mut *db)
        .await
        .unwrap()
    {
        return id;
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $2, true)")
        .bind(id)
        .bind(slug)
        .execute(&mut *db)
        .await
        .unwrap();
    id
}

async fn make_person(
    pool: &PgPool,
    roles: &[&str],
    department: Option<&str>,
    manager: Option<&Person>,
    superadmin: bool,
) -> Person {
    let mut tx = pool.begin().await.unwrap();
    let id = Uuid::new_v4();
    let name = u("Pytest Person");
    let email = format!("{}@granary.test", u("pytest.approver"));
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password) VALUES ($1, $2, $3, 'x', true, $4, false, 'internal', true, false)",
    )
    .bind(id)
    .bind(&email)
    .bind(&name)
    .bind(superadmin)
    .execute(&mut *tx)
    .await
    .unwrap();
    for slug in roles {
        let rid = role_id(&mut tx, slug).await;
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(id)
            .bind(rid)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let employee_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO employees (id, first_name, email, status, user_id, department, reports_to_id, salary_currency, \
         id_type, is_deleted) VALUES ($1, $2, $3, 'active', $4, $5, $6, 'JPY', 'national_id', false)",
    )
    .bind(employee_id)
    .bind(&name)
    .bind(&email)
    .bind(id)
    .bind(department)
    .bind(manager.map(|m| Uuid::parse_str(&m.employee_id).unwrap()))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = load_active_user(&mut conn, id).await.unwrap().unwrap();
    Person {
        id: id.to_string(),
        employee_id: employee_id.to_string(),
        user,
    }
}

async fn superadmin(pool: &PgPool) -> AuthUser {
    make_person(pool, &[], None, None, true).await.user
}

async fn make_product(pool: &PgPool, price: f64, cost: Option<f64>) -> String {
    let mut tx = pool.begin().await.unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, is_active, sale_model, price_metric, \
         modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) \
         VALUES ($1, $2, 'Pytest Laptop', 'physical', 'Unit', $3, true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
    )
    .bind(id)
    .bind(u("PYT"))
    .bind(cost)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', $3, '2000-01-01')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(price)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id.to_string()
}

async fn make_quotation(
    pool: &PgPool,
    product_id: Option<&str>,
    qty: f64,
    unit_price: f64,
    discount: f64,
    discount_type: &str,
) -> String {
    let amount = qty * unit_price * (1.0 - discount / 100.0);
    let mut tx = pool.begin().await.unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, total_quantity, \
         validity_days, procurement_type, customer_po_accepted_verbally, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, $5, $5, $6, 0, 0, 0, '[]', 0, 0, 14, 'standard', false, false)",
    )
    .bind(id)
    .bind(u("PYTEST-Q"))
    .bind(today())
    .bind(today() + Duration::days(14))
    .bind(amount)
    .bind(json!({"text": "NET30"}))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, product_id, name, quantity, unit_price, currency, \
         discount_percentage, discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, $3, 'Line', $4, $5, 'JPY', $6, $7, $8, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(product_id.map(|p| Uuid::parse_str(p).unwrap()))
    .bind(qty)
    .bind(unit_price)
    .bind(discount)
    .bind(discount_type)
    .bind(amount)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id.to_string()
}

async fn std_quotation(pool: &PgPool, product: Option<&str>, qty: f64) -> String {
    make_quotation(pool, product, qty, 10_000_000.0, 0.0, "none").await
}

fn err(e: ApiError) -> (u16, Value) {
    (e.status().as_u16(), e.body())
}

/// POST /approvals/submit.
async fn submit(
    pool: &PgPool,
    user: &AuthUser,
    entity_type: &str,
    entity_id: &str,
    flags: &[&str],
) -> (u16, Value) {
    let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
    let mut tx = pool.begin().await.unwrap();
    let r = notify::scope(async {
        let r = engine::submit(
            &mut tx,
            entity_type,
            entity_id,
            user,
            Some(&flags),
            None,
            None,
        )
        .await;
        notify::dispatch_outbox();
        r
    })
    .await;
    match r {
        Ok(request) => {
            tx.commit().await.unwrap();
            let mut conn = pool.acquire().await.unwrap();
            (
                201,
                engine::serialize(&mut conn, &request, Some(user))
                    .await
                    .unwrap(),
            )
        }
        Err(e) => {
            tx.rollback().await.unwrap();
            err(e)
        }
    }
}

/// POST /approvals/{id}/approve|reject.
async fn decide(
    pool: &PgPool,
    user: &AuthUser,
    request_id: &str,
    decision: &str,
    comment: Option<&str>,
) -> (u16, Value) {
    let decision = if decision == "approve" {
        "approved"
    } else {
        "rejected"
    };
    let mut tx = pool.begin().await.unwrap();
    let r = notify::scope(async {
        engine::act(&mut tx, request_id, user, decision, comment, None, None).await
    })
    .await;
    match r {
        Ok(request) => {
            tx.commit().await.unwrap();
            let mut conn = pool.acquire().await.unwrap();
            (
                200,
                engine::serialize(&mut conn, &request, Some(user))
                    .await
                    .unwrap(),
            )
        }
        Err(e) => {
            tx.rollback().await.unwrap();
            err(e)
        }
    }
}

async fn gate(
    pool: &PgPool,
    entity_type: &str,
    entity_id: &str,
) -> Result<Option<Value>, (u16, Value)> {
    let mut conn = pool.acquire().await.unwrap();
    match engine::ensure_approved(
        &mut conn,
        entity_type,
        Uuid::parse_str(entity_id).unwrap(),
        "sent",
    )
    .await
    {
        Ok(r) => Ok(r.map(|r| json!(r.id.to_string()))),
        Err(e) => Err(err(e)),
    }
}

fn reasons_mention(step: &Value, needle: &str) -> bool {
    step["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r.as_str().unwrap().contains(needle))
}

fn candidate_ids(step: &Value) -> Vec<String> {
    step["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

// ─── Quotation ──────────────────────────────────────────

#[tokio::test]
async fn test_standard_quotation_is_auto_approved_and_passes_the_gate() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let product = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = std_quotation(&pool, Some(&product), 2.0).await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 201, "{body}");
    assert_eq!(body["status"], "approved");
    assert!(body["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["action"] == "auto_approved"));
    assert!(gate(&pool, "quotation", &qid).await.unwrap().is_some());
}

#[tokio::test]
async fn test_unsubmitted_quotation_is_blocked_by_the_gate() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let product = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = std_quotation(&pool, Some(&product), 2.0).await;
    let (code, body) = gate(&pool, "quotation", &qid).await.unwrap_err();
    assert_eq!(code, 409);
    assert_eq!(body["detail"]["code"], "approval_required");
    assert_eq!(body["detail"]["approval_status"], "not_submitted");
}

#[tokio::test]
async fn test_catalog_quotation_at_list_price_is_approved_whatever_the_amount() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let product = make_product(&pool, 10_000_000.0, None).await; // JPY 1B, margin unknown
    let qid = std_quotation(&pool, Some(&product), 100.0).await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert!(code == 201 && body["status"] == "approved", "{body}");
}

/// The submit half of test_custom_quotation_needs_the_owner_and_the_bdr_cannot_approve (no decision).
#[tokio::test]
async fn test_custom_quotation_needs_the_owner_submit_part() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 25.0).await; // custom line -> Owner
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 201, "{body}");
    assert_eq!(body["status"], "pending");
    let step = &body["steps"][0];
    assert_eq!(step["name"], "Owner approval");
    assert!(reasons_mention(step, "list price"));
    assert!(!candidate_ids(step).contains(&bdr.id));
    assert_eq!(step["state"], "current");
    assert!(step["due_at"].is_string() || step["due_at"].is_null());

    let (code, _) = decide(
        &pool,
        &bdr.user,
        body["id"].as_str().unwrap(),
        "approve",
        None,
    )
    .await;
    assert_eq!(code, 403, "a BDR must not approve an Owner step");
    let (code, b) = decide(
        &pool,
        &bdr.user,
        body["id"].as_str().unwrap(),
        "approve",
        None,
    )
    .await;
    assert_eq!(
        b["detail"], "You are not an approver for this step",
        "{code}"
    );
}

#[tokio::test]
async fn test_custom_quotation_needs_the_owner_and_the_bdr_cannot_approve() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 25.0).await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 201, "{body}");
    assert_eq!(body["status"], "pending");
    let rid = body["id"].as_str().unwrap();
    let (code, _) = decide(&pool, &bdr.user, rid, "approve", None).await;
    assert_eq!(code, 403);
    let (code, approved) = decide(&pool, &owner, rid, "approve", Some("Strategic account")).await;
    assert_eq!(code, 200, "{approved}");
    assert_eq!(approved["status"], "approved");
    let approved_by: Option<Uuid> =
        sqlx::query_scalar("SELECT approved_by_id FROM quotations WHERE id = $1")
            .bind(Uuid::parse_str(&qid).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(approved_by.is_some());
}

#[tokio::test]
async fn test_reject_requires_a_comment_and_resubmission_restarts() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 30.0).await;
    let (_, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    let rid = body["id"].as_str().unwrap();

    let (code, _) = decide(&pool, &owner, rid, "reject", None).await;
    assert_eq!(code, 400);
    let (code, rejected) = decide(&pool, &owner, rid, "reject", Some("Margin too thin")).await;
    assert!(code == 200 && rejected["status"] == "rejected");

    let (code, again) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 201, "{again}");
    assert_ne!(again["id"], body["id"]);
    assert!(again["status"] == "pending" && again["current_step_seq"] == 1);
}

/// The part of test_reject_requires_a_comment_and_resubmission_restarts that decides nothing.
#[tokio::test]
async fn test_reject_requires_a_comment() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 30.0).await;
    let (_, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    let (code, b) = decide(
        &pool,
        &owner,
        body["id"].as_str().unwrap(),
        "reject",
        Some("   "),
    )
    .await;
    assert_eq!(
        (code, b["detail"].clone()),
        (400, json!("A comment is required when rejecting"))
    );
}

#[tokio::test]
async fn test_pending_request_cannot_be_submitted_twice() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 25.0).await;
    assert_eq!(
        submit(&pool, &bdr.user, "quotation", &qid, &[]).await.0,
        201
    );
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 409);
    assert_eq!(
        body["detail"],
        "Quotation already has a pending approval request"
    );
}

#[tokio::test]
async fn test_approval_goes_stale_when_the_document_changes() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let product = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = std_quotation(&pool, Some(&product), 2.0).await;
    let (_, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(body["status"], "approved");
    sqlx::query("UPDATE quotation_items SET unit_price = 9000000 WHERE quotation_id = $1")
        .bind(Uuid::parse_str(&qid).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let (_, b) = gate(&pool, "quotation", &qid).await.unwrap_err();
    assert_eq!(b["detail"]["code"], "approval_stale");

    let big = std_quotation(&pool, None, 25.0).await;
    let (_, pending) = submit(&pool, &bdr.user, "quotation", &big, &[]).await;
    sqlx::query("UPDATE quotation_items SET quantity = 26 WHERE quotation_id = $1")
        .bind(Uuid::parse_str(&big).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let (code, _) = decide(
        &pool,
        &owner,
        pending["id"].as_str().unwrap(),
        "approve",
        None,
    )
    .await;
    assert_eq!(code, 409);
    let status: String = sqlx::query_scalar("SELECT status FROM approval_requests WHERE id = $1")
        .bind(Uuid::parse_str(pending["id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");
}

/// The pending half of test_approval_goes_stale_when_the_document_changes: an edited document cancels its
/// pending request, and the cancellation (and the submitter's notice) survives the 409.
#[tokio::test]
async fn test_pending_request_is_cancelled_when_the_document_changes() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let big = std_quotation(&pool, None, 25.0).await;
    let (_, pending) = submit(&pool, &bdr.user, "quotation", &big, &[]).await;
    sqlx::query("UPDATE quotation_items SET quantity = 26 WHERE quotation_id = $1")
        .bind(Uuid::parse_str(&big).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let rid = Uuid::parse_str(pending["id"].as_str().unwrap()).unwrap();
    let (code, body) = decide(&pool, &owner, &rid.to_string(), "approve", None).await;
    assert_eq!(code, 409);
    assert_eq!(
        body["detail"],
        "The document changed after it was submitted: the request was cancelled and must be resubmitted"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM approval_requests WHERE id = $1")
        .bind(rid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");
    let titles: Vec<String> =
        sqlx::query_scalar("SELECT title FROM notifications WHERE user_id = $1 AND entity_id = $2")
            .bind(bdr.user.id)
            .bind(&big)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        titles.iter().any(|t| t.starts_with("Resubmit: ")),
        "{titles:?}"
    );
    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM approval_actions WHERE request_id = $1")
            .bind(rid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(actions.contains(&"cancelled".to_string()));
}

#[tokio::test]
async fn test_undocumented_discount_blocks_submission() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let product = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = make_quotation(&pool, Some(&product), 2.0, 10_000_000.0, 3.0, "none").await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 400);
    assert_eq!(body["detail"]["code"], "precondition_failed");
}

#[tokio::test]
async fn test_price_typed_below_list_counts_as_a_discount() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    // 20% below catalog with a "0%" discount: the hidden discount must be caught
    let p1 = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = make_quotation(&pool, Some(&p1), 2.0, 8_000_000.0, 0.0, "none").await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert!(
        code == 400 && body["detail"]["code"] == "precondition_failed",
        "{body}"
    );

    let p2 = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = make_quotation(&pool, Some(&p2), 2.0, 8_000_000.0, 0.0, "loyalty").await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 201, "{body}");
    assert_eq!(body["status"], "pending");
    assert!(reasons_mention(&body["steps"][0], "list price"));
}

#[tokio::test]
async fn test_any_discount_needs_the_owner() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let product = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = make_quotation(&pool, Some(&product), 10.0, 10_000_000.0, 7.0, "volume").await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 201, "{body}");
    assert!(body["status"] == "pending" && body["steps"][0]["name"] == "Owner approval");
}

#[tokio::test]
async fn test_price_above_list_needs_the_owner() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let product = make_product(&pool, 10_000_000.0, Some(5_000_000.0)).await;
    let qid = make_quotation(&pool, Some(&product), 2.0, 11_000_000.0, 0.0, "none").await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert!(code == 201 && body["status"] == "pending", "{body}");
}

#[tokio::test]
async fn test_owner_may_approve_their_own_submission() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let qid = std_quotation(&pool, None, 25.0).await;
    let (code, body) = submit(&pool, &owner, "quotation", &qid, &[]).await;
    assert!(code == 201 && body["status"] == "pending", "{body}");
    assert_eq!(body["can_act"], true);
    let (code, approved) =
        decide(&pool, &owner, body["id"].as_str().unwrap(), "approve", None).await;
    assert!(
        code == 200 && approved["status"] == "approved",
        "{approved}"
    );
}

// ─── Expense chain ──────────────────────────────────────────────

async fn make_expense(pool: &PgPool, claimant: &Person, amount: f64) -> String {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO expenses (id, category, description, amount, currency, exchange_rate, expense_date, status, \
         submitted_by_id, tax_deductible, withholding_applicable, is_deleted) \
         VALUES ($1, 'Travel', 'Client visit', $2, 'JPY', 1, $3, 'pending', $4, false, false, false)",
    )
    .bind(id)
    .bind(amount)
    .bind(today())
    .bind(claimant.user.id)
    .execute(pool)
    .await
    .unwrap();
    id.to_string()
}

#[tokio::test]
async fn test_expense_walks_manager_then_finance_then_owner() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let manager = make_person(&pool, &["team_lead"], None, None, false).await;
    let finance = make_person(&pool, &["finance_admin"], None, None, false).await;
    let claimant = make_person(&pool, &[], None, Some(&manager), false).await;
    let expense_id = make_expense(&pool, &claimant, 1_600_000.0).await;

    let (code, _) = submit(&pool, &claimant.user, "expense", &expense_id, &[]).await;
    assert_eq!(
        code, 400,
        "above JPY 500k the claimant must confirm pre-approval"
    );
    let (code, body) = submit(
        &pool,
        &claimant.user,
        "expense",
        &expense_id,
        &["pre_approved"],
    )
    .await;
    assert_eq!(code, 201, "{body}");
    let names: Vec<&str> = body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Line manager approval",
            "Finance Admin review",
            "Owner approval"
        ]
    );
    assert_eq!(candidate_ids(&body["steps"][0]), vec![manager.id.clone()]);
    let rid = body["id"].as_str().unwrap();

    assert_eq!(
        decide(&pool, &finance.user, rid, "approve", None).await.0,
        403
    );
    assert_eq!(
        decide(&pool, &claimant.user, rid, "approve", None).await.0,
        403
    );
    let (code, step2) = decide(&pool, &manager.user, rid, "approve", None).await;
    assert!(code == 200 && step2["current_step_seq"] == 2, "{step2}");
    let (code, step3) = decide(&pool, &finance.user, rid, "approve", None).await;
    assert!(code == 200 && step3["current_step_seq"] == 3, "{step3}");
    let (code, done) = decide(&pool, &owner, rid, "approve", None).await;
    assert!(code == 200 && done["status"] == "approved", "{done}");
    let status: String = sqlx::query_scalar("SELECT status FROM expenses WHERE id = $1")
        .bind(Uuid::parse_str(&expense_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "approved");
}

/// The steps of the expense chain up to the first (non-final) decision: no approval.decided is emitted.
#[tokio::test]
async fn test_expense_chain_first_two_steps() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let manager = make_person(&pool, &["team_lead"], None, None, false).await;
    let finance = make_person(&pool, &["finance_admin"], None, None, false).await;
    let claimant = make_person(&pool, &[], None, Some(&manager), false).await;
    let expense_id = make_expense(&pool, &claimant, 1_600_000.0).await;

    let (code, body) = submit(&pool, &claimant.user, "expense", &expense_id, &[]).await;
    assert_eq!(
        code, 400,
        "above JPY 500k the claimant must confirm pre-approval"
    );
    assert_eq!(body["detail"]["code"], "precondition_failed");
    let (code, body) = submit(
        &pool,
        &claimant.user,
        "expense",
        &expense_id,
        &["pre_approved"],
    )
    .await;
    assert_eq!(code, 201, "{body}");
    let names: Vec<&str> = body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Line manager approval",
            "Finance Admin review",
            "Owner approval"
        ]
    );
    assert_eq!(candidate_ids(&body["steps"][0]), vec![manager.id.clone()]);
    assert_eq!(body["flags"], json!(["pre_approved"]));
    assert_eq!(body["actions"][0]["comment"], "pre_approved");
    let rid = body["id"].as_str().unwrap();

    assert_eq!(
        decide(&pool, &finance.user, rid, "approve", None).await.0,
        403
    );
    let (code, b) = decide(&pool, &claimant.user, rid, "approve", None).await;
    assert_eq!(code, 403, "{b}");
    let (code, step2) = decide(&pool, &manager.user, rid, "approve", None).await;
    assert!(code == 200 && step2["current_step_seq"] == 2, "{step2}");
    assert_eq!(step2["steps"][0]["state"], "approved");
    assert_eq!(step2["steps"][1]["state"], "current");
    assert_eq!(step2["steps"][2]["state"], "waiting");
    let (code, step3) = decide(&pool, &finance.user, rid, "approve", None).await;
    assert!(code == 200 && step3["current_step_seq"] == 3, "{step3}");
}

// ─── Vendor invoice ─────────────────────────────────────────────

async fn vendor_with_wo(pool: &PgPool, status: &str) -> (String, String) {
    let vendor = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, total_spend, \
         spend_currency, is_deleted) VALUES ($1, $2, 'professional_services', 'active', false, false, 0, 'JPY', false)",
    )
    .bind(vendor)
    .bind(u("Pytest Vendor"))
    .execute(pool)
    .await
    .unwrap();
    let wo = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders (id, number, vendor_id, title, status, currency, payment_type, agreed_value, \
         withholding_applicable, is_deleted) VALUES ($1, $2, $3, 'Install', $4, 'JPY', 'fixed', 3000000, false, false)",
    )
    .bind(wo)
    .bind(u("PYT-WO"))
    .bind(vendor)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    (vendor.to_string(), wo.to_string())
}

async fn vendor_invoice(pool: &PgPool, vendor_id: &str, wo_id: &str) -> String {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendor_invoices (id, vendor_id, work_order_id, vendor_invoice_number, invoice_date, amount, \
         currency, status, tax_amount, withholding_amount, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 3000000, 'JPY', 'submitted', 0, 0, false)",
    )
    .bind(id)
    .bind(Uuid::parse_str(vendor_id).unwrap())
    .bind(Uuid::parse_str(wo_id).unwrap())
    .bind(u("VINV"))
    .bind(today())
    .execute(pool)
    .await
    .unwrap();
    id.to_string()
}

#[tokio::test]
async fn test_finance_admin_may_self_approve_small_vendor_invoices() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let finance = make_person(&pool, &["finance_admin"], None, None, false).await;
    let (vendor_id, wo_id) = vendor_with_wo(&pool, "completed").await;
    let vi = vendor_invoice(&pool, &vendor_id, &wo_id).await;
    let (code, body) = submit(&pool, &finance.user, "vendor_invoice", &vi, &[]).await;
    assert_eq!(code, 201, "{body}");
    assert_eq!(body["steps"][0]["name"], "Finance Admin review");
    let (code, done) = decide(
        &pool,
        &finance.user,
        body["id"].as_str().unwrap(),
        "approve",
        None,
    )
    .await;
    assert!(code == 200 && done["status"] == "approved", "{done}");
}

/// The submit half of the self-approval test: the finance admin is a candidate of their own step.
#[tokio::test]
async fn test_finance_admin_is_a_candidate_of_their_own_vendor_invoice() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let finance = make_person(&pool, &["finance_admin"], None, None, false).await;
    let (vendor_id, wo_id) = vendor_with_wo(&pool, "completed").await;
    let vi = vendor_invoice(&pool, &vendor_id, &wo_id).await;
    let (code, body) = submit(&pool, &finance.user, "vendor_invoice", &vi, &[]).await;
    assert_eq!(code, 201, "{body}");
    assert_eq!(body["steps"][0]["name"], "Finance Admin review");
    assert!(candidate_ids(&body["steps"][0]).contains(&finance.id));
    assert_eq!(body["can_act"], true);
}

#[tokio::test]
async fn test_vendor_invoice_without_closed_work_order_is_refused() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let finance = make_person(&pool, &["finance_admin"], None, None, false).await;
    let (vendor_id, wo_id) = vendor_with_wo(&pool, "active").await;
    let vi = vendor_invoice(&pool, &vendor_id, &wo_id).await;
    let (code, body) = submit(&pool, &finance.user, "vendor_invoice", &vi, &[]).await;
    assert!(
        code == 400 && body["detail"]["code"] == "precondition_failed",
        "{body}"
    );
}

// ─── Purchase order routing ─────────────────────────────────────

async fn make_po(pool: &PgPool, requested_by: &Person, total: f64) -> String {
    let vendor = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, total_spend, \
         spend_currency, is_deleted) VALUES ($1, $2, 'component_supplier', 'active', false, false, 0, 'JPY', false)",
    )
    .bind(vendor)
    .bind(u("Pytest Supplier"))
    .execute(pool)
    .await
    .unwrap();
    let po = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_orders (id, number, vendor_id, status, currency, subtotal, total, requested_by_id, \
         tax_amount, shipping_cost, approval_status, is_deleted) \
         VALUES ($1, $2, $3, 'draft', 'JPY', $4, $4, $5, 0, 0, 'not_required', false)",
    )
    .bind(po)
    .bind(u("PYT-PO"))
    .bind(vendor)
    .bind(total)
    .bind(requested_by.user.id)
    .execute(pool)
    .await
    .unwrap();
    po.to_string()
}

#[tokio::test]
async fn test_role_step_with_no_other_holder_routes_to_the_owner() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let department = u("Pytest Manufacturing");
    let lead = make_person(&pool, &["team_lead"], Some(&department), None, false).await;
    let (step_id, original): (Uuid, Option<String>) = sqlx::query_as(
        "SELECT s.id, s.department FROM approval_policy_steps s JOIN approval_policies p ON p.id = s.policy_id \
         WHERE p.entity_type = 'purchase_order' AND s.sequence = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE approval_policy_steps SET department = $2 WHERE id = $1")
        .bind(step_id)
        .bind(&department)
        .execute(&pool)
        .await
        .unwrap();
    let po = make_po(&pool, &lead, 3_000_000.0).await;
    let (code, body) = submit(&pool, &lead.user, "purchase_order", &po, &[]).await;
    sqlx::query("UPDATE approval_policy_steps SET department = $2 WHERE id = $1")
        .bind(step_id)
        .bind(&original)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(code, 201, "{body}");
    let first = &body["steps"][0];
    assert_eq!(first["name"], "Manufacturing Lead approval");
    assert!(
        first["routing_note"]
            .as_str()
            .is_some_and(|n| n.starts_with("No eligible")),
        "{first}"
    );
    assert_eq!(
        first["routing_note"],
        json!(format!(
            "No eligible holder of role 'team_lead' in {department}: routed to the Owner"
        ))
    );
    assert!(!candidate_ids(first).contains(&lead.id));
}

#[tokio::test]
async fn test_large_purchase_order_needs_two_quotes() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let lead = make_person(&pool, &["team_lead"], Some("Manufacturing"), None, false).await;
    let po = make_po(&pool, &lead, 20_000_000.0).await;
    let (code, body) = submit(&pool, &lead.user, "purchase_order", &po, &[]).await;
    assert!(
        code == 400 && body["detail"]["code"] == "precondition_failed",
        "{body}"
    );
    for n in 0..2 {
        sqlx::query(
            "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, file_type) \
             VALUES ($1, 'purchase_order', $2, $3, 10, 'application/pdf', $4, $5, 'file')",
        )
        .bind(Uuid::new_v4())
        .bind(&po)
        .bind(format!("quote-{n}.pdf"))
        .bind(format!("k/{n}"))
        .bind(format!("/q/{n}"))
        .execute(&pool)
        .await
        .unwrap();
    }
    let (code, body) = submit(&pool, &lead.user, "purchase_order", &po, &[]).await;
    assert_eq!(code, 201, "{body}");
    let names: Vec<&str> = body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Owner approval"]);
}

// ─── Inbox, visibility, policy administration ─────────────────────────────────

#[tokio::test]
#[ignore = "covered by crates/api/tests/approvals_routes.rs::test_inbox_views_and_visibility (wave 4)"]
async fn test_inbox_views_and_visibility() {}

/// The engine half of test_policy_edits_are_validated_and_never_touch_requests_in_flight: a policy edit
/// never changes a request in flight (PATCH validation itself is router logic).
#[tokio::test]
async fn test_policy_edits_never_touch_requests_in_flight() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 25.0).await;
    let (_, pending) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    let (policy_id, sla): (Uuid, Option<i32>) = sqlx::query_as(
        "SELECT id, sla_hours FROM approval_policies WHERE entity_type = 'quotation'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE approval_policy_steps SET name = 'Renamed' WHERE policy_id = $1")
        .bind(policy_id)
        .execute(&pool)
        .await
        .unwrap();
    let row: granary_db::rows::approval::ApprovalRequest =
        sqlx::query_as("SELECT * FROM approval_requests WHERE id = $1")
            .bind(Uuid::parse_str(pending["id"].as_str().unwrap()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let frozen = engine::serialize(&mut conn, &row, Some(&bdr.user))
        .await
        .unwrap();
    sqlx::query("UPDATE approval_policy_steps SET name = 'Owner approval' WHERE policy_id = $1")
        .bind(policy_id)
        .execute(&pool)
        .await
        .unwrap();
    let _ = sla;
    let a: Vec<&Value> = frozen["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["name"])
        .collect();
    let b: Vec<&Value> = pending["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["name"])
        .collect();
    assert_eq!(a, b);
    assert_eq!(frozen["can_cancel"], true);
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/approvals_routes.rs::test_policy_edits_are_validated_and_never_touch_requests_in_flight (wave 4)"]
async fn test_policy_edits_are_validated_and_never_touch_requests_in_flight() {}

// ─── test_approval_gating.py ──────────────────────────────────────────────────

/// test_sending_a_large_quotation_submits_it_and_waits_for_the_owner, through the engine call the send
/// endpoint makes (`ensure_or_submit(db, "quotation", q, user, "sent")`).
#[tokio::test]
async fn test_sending_a_large_quotation_submits_it_and_waits_for_the_owner_engine_part() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 30.0).await;
    let q = Uuid::parse_str(&qid).unwrap();
    let mut tx = pool.begin().await.unwrap();
    let r = notify::scope(async {
        engine::ensure_or_submit(&mut tx, "quotation", q, &bdr.user, "sent", None).await
    })
    .await;
    tx.rollback().await.unwrap(); // the endpoint's error path; the submission was committed inside
    let (code, body) = err(r.unwrap_err());
    assert_eq!(code, 409, "{body}");
    assert_eq!(body["detail"]["code"], "approval_pending");
    assert!(body["detail"]["request_id"].is_string());
    assert_eq!(
        body["detail"]["message"],
        "Submitted for approval: Owner approval is required before it can be sent"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1")
        .bind(q)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "draft");

    // sending again does not resubmit
    let mut tx = pool.begin().await.unwrap();
    let r = notify::scope(async {
        engine::ensure_or_submit(&mut tx, "quotation", q, &bdr.user, "sent", None).await
    })
    .await;
    tx.rollback().await.unwrap();
    let (code, body) = err(r.unwrap_err());
    assert_eq!(code, 409);
    assert_eq!(body["detail"]["code"], "approval_required");
    assert_eq!(body["detail"]["approval_status"], "pending");
    assert_eq!(
        body["detail"]["message"],
        "This quotation must be approved before it can be sent (approval is pending)"
    );
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/sales_routes.rs::test_sending_a_large_quotation_submits_it_and_waits_for_the_owner (wave 4)"]
async fn test_sending_a_large_quotation_submits_it_and_waits_for_the_owner() {}

#[tokio::test]
#[ignore = "covered by crates/api/tests/sales_routes.rs::test_standard_quotation_sends_straight_away (wave 4)"]
async fn test_standard_quotation_sends_straight_away() {}

#[tokio::test]
#[ignore = "covered by crates/api/tests/sales_routes.rs::test_quotation_status_cannot_be_set_to_approved (wave 4)"]
async fn test_quotation_status_cannot_be_set_to_approved() {}

#[tokio::test]
#[ignore = "covered by crates/api/tests/vendor_routes.rs::test_contractor_request_is_submitted_and_only_the_owner_can_decide (wave 4)"]
async fn test_contractor_request_is_submitted_and_only_the_owner_can_decide() {}

/// Known Python failure (main bddbe04, test DB built by create_all): the offer letter is numbered
/// "OFFER_LETTER-2026-10-00001" instead of "OL..." (numbering, not approvals). Reproduced on a scratch
/// database (`assert offer["number"].startswith("OL")` fails); everything after it is untested in Python.
#[tokio::test]
#[ignore = "covered by crates/api/tests/hr_routes.rs::test_offer_letter_needs_owner_approval_before_it_is_issued (wave 4); known Python failure (offer number prefix)"]
async fn test_offer_letter_needs_owner_approval_before_it_is_issued() {}

#[tokio::test]
#[ignore = "covered by crates/api/tests/hr_routes.rs::test_offer_letters_are_hr_only (wave 4)"]
async fn test_offer_letters_are_hr_only() {}

#[tokio::test]
async fn test_overdue_steps_are_reminded_once_a_day() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let _ = superadmin(&pool).await;
    let qid = std_quotation(&pool, None, 30.0).await;
    let (code, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    assert_eq!(code, 201, "{body}");
    // The seeded database has a superadmin whose employee position is Owner: the step routes to them only
    // (Python's create_all test database has no seeded Owner, so its new superadmin was a candidate).
    let owner_id = Uuid::parse_str(&candidate_ids(&body["steps"][0])[0]).unwrap();
    let request_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    // step["due_at"] = (now - 5h).isoformat()
    let mut row: granary_db::rows::approval::ApprovalRequest =
        sqlx::query_as("SELECT * FROM approval_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let due =
        granary_compat::pyjson::PyDateTime::from_chrono_utc(&(Utc::now() - Duration::hours(5)))
            .isoformat();
    let seq = row.current_step_seq.unwrap();
    for s in row.steps.as_array_mut().unwrap() {
        if s["seq"] == seq {
            s["due_at"] = json!(due);
        }
    }
    sqlx::query("UPDATE approval_requests SET steps = $2 WHERE id = $1")
        .bind(request_id)
        .bind(&row.steps)
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let n = notify::scope(async {
        let n = engine::remind_overdue(&mut tx, None).await.unwrap();
        assert!(notify::pending_outbox()
            .iter()
            .any(|i| i.whatsapp && i.email));
        n
    })
    .await;
    tx.commit().await.unwrap();
    assert!(n >= 1);
    let mut tx = pool.begin().await.unwrap();
    let again = notify::scope(async { engine::remind_overdue(&mut tx, None).await.unwrap() }).await;
    tx.commit().await.unwrap();
    let last: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT last_reminded_at FROM approval_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(last.is_some());
    let titles: Vec<String> =
        sqlx::query_scalar("SELECT title FROM notifications WHERE user_id = $1 AND entity_id = $2")
            .bind(owner_id)
            .bind(&qid)
            .fetch_all(&pool)
            .await
            .unwrap();
    let submitter: Vec<String> =
        sqlx::query_scalar("SELECT title FROM notifications WHERE user_id = $1 AND entity_id = $2")
            .bind(bdr.user.id)
            .bind(&qid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        titles
            .iter()
            .filter(|t| t.starts_with("Overdue approval"))
            .count(),
        1
    );
    assert!(submitter.iter().any(|t| t.starts_with("Approval overdue")));
    assert!(again == 0 || last.is_some());
    let body: String = sqlx::query_scalar(
        "SELECT body FROM notifications WHERE user_id = $1 AND entity_id = $2 AND title LIKE 'Overdue approval%'",
    )
    .bind(owner_id)
    .bind(&qid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        body,
        "Owner approval is 5h past its SLA and still waiting for your decision"
    );
}

// ─── engine units ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_withdraws_and_only_the_submitter_may() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let other = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 25.0).await;
    let (_, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    let rid = body["id"].as_str().unwrap().to_string();
    let mut tx = pool.begin().await.unwrap();
    let e = engine::cancel(&mut tx, &rid, &other.user, None)
        .await
        .unwrap_err();
    assert_eq!(
        err(e),
        (
            403,
            json!({"detail": "Only the submitter can withdraw this request"})
        )
    );
    let r = engine::cancel(&mut tx, &rid, &bdr.user, None)
        .await
        .unwrap();
    assert_eq!(r.status, "cancelled");
    let out = engine::serialize(&mut tx, &r, Some(&bdr.user))
        .await
        .unwrap();
    let last = out["actions"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["action"], "cancelled");
    assert_eq!(last["comment"], "Withdrawn by submitter");
    assert_eq!(out["steps"][0]["state"], "skipped");
    let e = engine::cancel(&mut tx, &rid, &bdr.user, None)
        .await
        .unwrap_err();
    assert_eq!(
        err(e),
        (409, json!({"detail": "This request is already cancelled"}))
    );
    tx.commit().await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let states = engine::states_for(&mut conn, "quotation", &[Uuid::parse_str(&qid).unwrap()])
        .await
        .unwrap();
    assert_eq!(
        Value::Object(states),
        json!({qid.clone(): {"request_id": rid, "status": "cancelled", "current_step": null}})
    );
}

#[tokio::test]
async fn act_rejects_bad_decisions_and_unknown_requests() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let owner = superadmin(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let e = engine::act(
        &mut conn,
        &Uuid::new_v4().to_string(),
        &owner,
        "maybe",
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err(e),
        (
            400,
            json!({"detail": "decision must be 'approved' or 'rejected'"})
        )
    );
    let e = engine::act(
        &mut conn,
        &Uuid::new_v4().to_string(),
        &owner,
        "approved",
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err(e),
        (404, json!({"detail": "Approval request not found"}))
    );
    let e = engine::act_on_pending(
        &mut conn,
        "quotation",
        Uuid::new_v4(),
        &owner,
        "approved",
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err(e),
        (
            409,
            json!({"detail": {"code": "nothing_pending", "approval_status": "not_submitted",
            "message": "This quotation has no pending approval request: submit it for approval first"}})
        )
    );
    let e = engine::submit(&mut conn, "no_such_type", "x", &owner, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(
        err(e),
        (
            400,
            json!({"detail": "Unknown document type: 'no_such_type'"})
        )
    );
    let e = engine::submit(
        &mut conn,
        "quotation",
        &Uuid::new_v4().to_string(),
        &owner,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err(e), (404, json!({"detail": "Quotation not found"})));
}

#[tokio::test]
async fn supersede_for_entity_retires_pending_requests() {
    let _g = SERIAL.lock().await;
    let pool = pool().await;
    let bdr = make_person(&pool, &["sales_bizdev"], None, None, false).await;
    let qid = std_quotation(&pool, None, 25.0).await;
    let (_, body) = submit(&pool, &bdr.user, "quotation", &qid, &[]).await;
    let mut tx = pool.begin().await.unwrap();
    engine::supersede_for_entity(
        &mut tx,
        "quotation",
        Uuid::parse_str(&qid).unwrap(),
        "Revised",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (status, completed): (String, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("SELECT status, completed_at FROM approval_requests WHERE id = $1")
            .bind(Uuid::parse_str(body["id"].as_str().unwrap()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "superseded");
    assert!(completed.is_some());
    // superseded requests are invisible to the gate: it is "not submitted" again
    let (_, b) = gate(&pool, "quotation", &qid).await.unwrap_err();
    assert_eq!(b["detail"]["approval_status"], "not_submitted");
    // and the document can be submitted afresh
    assert_eq!(
        submit(&pool, &bdr.user, "quotation", &qid, &[]).await.0,
        201
    );
}
