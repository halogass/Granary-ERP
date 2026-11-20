//! routers/approvals.py through the real app. Ports of the backend tests that seed documents directly:
//! - test_approval_engine.py: test_inbox_views_and_visibility,
//!   test_policy_edits_are_validated_and_never_touch_requests_in_flight (the two services tests
//!   crates/services/tests/approvals_engine.rs left `#[ignore = "routers wave: ..."]`);
//! - test_audit_medium_w3.py: test_withdrawing_an_approval_puts_the_document_back,
//!   test_an_edited_document_cancels_its_request_and_tells_the_submitter,
//!   test_the_document_approval_panel_hides_requests_from_uninvolved_users;
//! - test_approval_gating.py: test_overdue_steps_are_reminded_once_a_day (submit through the API).

mod support;

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{u, TestApp, TestUser, UserSpec};
use uuid::Uuid;

const TAG: &str = "approvals";

fn app() -> &'static TestApp {
    support::app(TAG)
}

/// `make_person(roles=..., perms=("approvals:view",))` with an employee record.
async fn person(app: &TestApp, roles: &[&str], perms: &[&str]) -> TestUser {
    let user = app
        .make_user(UserSpec::new().perms(perms).roles(roles))
        .await;
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO employees (id, first_name, email, status, user_id, salary_currency, id_type, is_deleted) \
         VALUES ($1, $2, $3, 'active', $4, 'JPY', 'national_id', false)",
    )
    .bind(Uuid::new_v4())
    .bind(u("Pytest Person"))
    .bind(&user.email)
    .bind(user.id)
    .execute(&mut db)
    .await
    .unwrap();
    user
}

/// `make_quotation(product_id=None, qty=25)`: a custom line, which routes to the Owner.
async fn custom_quotation(db: &mut PgConnection, qty: f64) -> Uuid {
    let today = Utc::now().date_naive();
    let amount = qty * 10_000_000.0;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, total_quantity, \
         validity_days, procurement_type, customer_po_accepted_verbally, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, $5, $5, $6, 0, 0, 0, '[]', 0, 0, 14, 'standard', false, false)",
    )
    .bind(id)
    .bind(u("PYTEST-Q"))
    .bind(today)
    .bind(today + Duration::days(14))
    .bind(amount)
    .bind(json!({"text": "NET30"}))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, product_id, name, quantity, unit_price, currency, \
         discount_percentage, discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, NULL, 'Line', $3, 10000000, 'JPY', 0, 'none', $4, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(qty)
    .bind(amount)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// `_vendor_invoice()`: a submitted vendor invoice on a completed work order.
async fn vendor_invoice(db: &mut PgConnection) -> Uuid {
    let vendor = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, total_spend, \
         spend_currency, is_deleted) VALUES ($1, $2, 'professional_services', 'active', false, false, 0, 'JPY', false)",
    )
    .bind(vendor)
    .bind(u("Pytest W3 Vendor"))
    .execute(&mut *db)
    .await
    .unwrap();
    let wo = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders (id, number, vendor_id, title, status, currency, payment_type, agreed_value, \
         withholding_applicable, is_deleted) VALUES ($1, $2, $3, 'Install', 'completed', 'JPY', 'fixed', 3000000, false, false)",
    )
    .bind(wo)
    .bind(u("PYT-WO"))
    .bind(vendor)
    .execute(&mut *db)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendor_invoices (id, vendor_id, work_order_id, vendor_invoice_number, invoice_date, amount, \
         currency, status, tax_amount, withholding_amount, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 3000000, 'JPY', 'submitted', 0, 0, false)",
    )
    .bind(id)
    .bind(vendor)
    .bind(wo)
    .bind(u("VINV"))
    .bind(Utc::now().date_naive())
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn purchase_order(db: &mut PgConnection, requested_by: Uuid) -> Uuid {
    let vendor = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, total_spend, \
         spend_currency, is_deleted) VALUES ($1, $2, 'component_supplier', 'active', false, false, 0, 'JPY', false)",
    )
    .bind(vendor)
    .bind(u("Pytest W3 Supplier"))
    .execute(&mut *db)
    .await
    .unwrap();
    let po = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_orders (id, number, vendor_id, status, currency, subtotal, total, requested_by_id, \
         tax_amount, shipping_cost, approval_status, is_deleted) \
         VALUES ($1, $2, $3, 'draft', 'JPY', 7000000, 7000000, $4, 0, 0, 'not_required', false)",
    )
    .bind(po)
    .bind(u("PYT-PO"))
    .bind(vendor)
    .bind(requested_by)
    .execute(&mut *db)
    .await
    .unwrap();
    for amount in [3_000_000.0f64, 4_000_000.0] {
        sqlx::query(
            "INSERT INTO purchase_order_items (id, po_id, description, quantity_ordered, unit_price, currency, amount, \
             quantity_received) VALUES ($1, $2, 'Part', 1, $3, 'JPY', $3, 0)",
        )
        .bind(Uuid::new_v4())
        .bind(po)
        .bind(amount)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    po
}

async fn submit(app: &TestApp, token: &str, entity_type: &str, id: Uuid) -> support::Resp {
    app.post(
        token,
        "/api/v1/approvals/submit",
        json!({"entity_type": entity_type, "entity_id": id.to_string()}),
    )
    .await
}

async fn status_of(db: &mut PgConnection, table: &str, id: Uuid) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT status FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_inbox_views_and_visibility() {
    let app = app();
    let sa = app.superadmin().await;
    let bdr = person(app, &["sales_bizdev"], &["approvals:view"]).await;
    let outsider = person(app, &[], &["approvals:view"]).await;
    let no_view = person(app, &[], &[]).await;
    let mut db = app.db().await;
    let qid = custom_quotation(&mut db, 25.0).await;
    let r = submit(app, &bdr.token, "quotation", qid).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let body = r.json();
    assert_eq!(body["status"], "pending");

    let mine = app
        .get(&bdr.token, "/api/v1/approvals?scope=submitted_by_me")
        .await
        .json();
    assert!(mine
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == body["id"]));
    let involved = app
        .get(&bdr.token, "/api/v1/approvals?scope=involved")
        .await
        .json();
    assert!(involved
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == body["id"]));
    let trail = app
        .get(
            &bdr.token,
            &format!("/api/v1/approvals/entity/quotation/{qid}"),
        )
        .await
        .json();
    assert_eq!(trail["gate"], "pending");
    assert_eq!(trail["request"]["id"], body["id"]);
    assert_eq!(trail["history"].as_array().unwrap().len(), 1);
    let keys: Vec<&str> = trail["history"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "status",
            "document_revision",
            "submitted_at",
            "completed_at"
        ]
    );
    assert_eq!(trail["policy"]["signature_flow"], "none");
    assert!(trail["policy"]["flags"].is_array());

    let r = app
        .get(
            &outsider.token,
            &format!("/api/v1/approvals/{}", body["id"].as_str().unwrap()),
        )
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json(),
        json!({"detail": "You are not involved in this approval"})
    );
    let r = app
        .get(&no_view.token, "/api/v1/approvals?scope=submitted_by_me")
        .await;
    assert_eq!(r.status, 403);
    let count = app.get(&sa.token, "/api/v1/approvals/count").await.json();
    assert!(count["awaiting_me"].is_i64());
    // the submitter sees the request but is not awaited on it
    let got = app
        .get(
            &bdr.token,
            &format!("/api/v1/approvals/{}", body["id"].as_str().unwrap()),
        )
        .await;
    assert_eq!(got.status, 200);
    assert_eq!(got.json()["id"], body["id"]);
    assert_eq!(
        app.get(&bdr.token, "/api/v1/approvals/count").await.json(),
        json!({"awaiting_me": 0})
    );
    let all = app
        .get(
            &sa.token,
            "/api/v1/approvals?scope=all&entity_type=quotation&limit=500",
        )
        .await
        .json();
    assert!(all
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == body["id"]));
    // the outsider cannot act on it either
    let r = app
        .post(
            &outsider.token,
            &format!("/api/v1/approvals/{}/approve", body["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn test_policy_edits_are_validated_and_never_touch_requests_in_flight() {
    let app = app();
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let bdr = person(app, &["sales_bizdev"], &["approvals:view"]).await;
    let mut db = app.db().await;
    let qid = custom_quotation(&mut db, 25.0).await;
    let pending = submit(app, &bdr.token, "quotation", qid).await.json();

    let policies = app
        .get(&sa.token, "/api/v1/approvals/policies")
        .await
        .json();
    let quotation = policies
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["entity_type"] == "quotation")
        .unwrap()
        .clone();
    let pid = quotation["id"].as_str().unwrap();
    let bad = app
        .patch(
            &sa.token,
            &format!("/api/v1/approvals/policies/{pid}"),
            json!({"steps": [{"sequence": 1, "name": "X", "approver_mode": "owner", "condition": {"rule": "bogus"}}]}),
        )
        .await;
    assert_eq!(bad.status, 400, "{}", bad.text());

    let original_sla = quotation["sla_hours"].clone();
    let ok = app
        .patch(
            &sa.token,
            &format!("/api/v1/approvals/policies/{pid}"),
            json!({"sla_hours": 12}),
        )
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(
        ok.json()["version"],
        quotation["version"].as_i64().unwrap() + 1
    );
    let updated_by: Option<Uuid> =
        sqlx::query_scalar("SELECT updated_by_id FROM approval_policies WHERE id = $1")
            .bind(Uuid::parse_str(pid).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(updated_by, Some(sa.id));

    let frozen = app
        .get(
            &sa.token,
            &format!("/api/v1/approvals/{}", pending["id"].as_str().unwrap()),
        )
        .await
        .json();
    let names = |v: &Value| -> Vec<Value> {
        v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].clone())
            .collect()
    };
    assert_eq!(names(&frozen), names(&pending));
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/approvals/policies/{pid}"),
            json!({"sla_hours": original_sla}),
        )
        .await;
    assert_eq!(r.status, 200);
    // an explicit null on a NOT NULL column is the IntegrityError 409
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/approvals/policies/{pid}"),
            json!({"name": null}),
        )
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    let r = app
        .patch(
            &sa.token,
            "/api/v1/approvals/policies/not-a-uuid",
            json!({}),
        )
        .await;
    assert_eq!(r.status, 422);
    assert_eq!(r.json()["errors"][0]["field"], "policy_id");
}

#[tokio::test]
async fn test_withdrawing_an_approval_puts_the_document_back() {
    let app = app();
    let submitter = person(app, &[], &["approvals:view"]).await;
    let mut db = app.db().await;
    let vid = vendor_invoice(&mut db).await;
    let r = submit(app, &submitter.token, "vendor_invoice", vid).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(
        status_of(&mut db, "vendor_invoices", vid).await.as_deref(),
        Some("under_review")
    );
    let rid = r.json()["id"].as_str().unwrap().to_string();
    let c = app
        .post(
            &submitter.token,
            &format!("/api/v1/approvals/{rid}/cancel"),
            json!({}),
        )
        .await;
    assert_eq!(c.status, 200, "{}", c.text());
    assert_eq!(c.json()["status"], "cancelled");
    assert_eq!(
        status_of(&mut db, "vendor_invoices", vid).await.as_deref(),
        Some("submitted")
    );
    let again = app
        .post(
            &submitter.token,
            &format!("/api/v1/approvals/{rid}/cancel"),
            json!({}),
        )
        .await;
    assert_eq!(again.status, 409);
    assert_eq!(
        again.json(),
        json!({"detail": "This request is already cancelled"})
    );

    let pid = purchase_order(&mut db, submitter.id).await;
    let r = submit(app, &submitter.token, "purchase_order", pid).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let c = app
        .post(
            &submitter.token,
            &format!(
                "/api/v1/approvals/{}/cancel",
                r.json()["id"].as_str().unwrap()
            ),
            json!({"reason": "changed my mind"}),
        )
        .await;
    assert_eq!(c.status, 200, "{}", c.text());
    let approval_status: Option<String> =
        sqlx::query_scalar("SELECT approval_status FROM purchase_orders WHERE id = $1")
            .bind(pid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(approval_status.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn test_an_edited_document_cancels_its_request_and_tells_the_submitter() {
    let app = app();
    let sa = app.superadmin().await;
    let submitter = person(app, &[], &["approvals:view"]).await;
    let mut db = app.db().await;
    let vid = vendor_invoice(&mut db).await;
    let r = submit(app, &submitter.token, "vendor_invoice", vid).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let rid = Uuid::parse_str(r.json()["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE vendor_invoices SET amount = 3500000 WHERE id = $1")
        .bind(vid)
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/approvals/{rid}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(
        status_of(&mut db, "approval_requests", rid)
            .await
            .as_deref(),
        Some("cancelled")
    );
    assert_eq!(
        status_of(&mut db, "vendor_invoices", vid).await.as_deref(),
        Some("submitted")
    );
    let notes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND title LIKE 'Resubmit:%'",
    )
    .bind(submitter.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(notes > 0);
}

#[tokio::test]
async fn test_the_document_approval_panel_hides_requests_from_uninvolved_users() {
    let app = app();
    let submitter = person(app, &[], &["approvals:view"]).await;
    let outsider = person(app, &[], &["approvals:view"]).await;
    let mut db = app.db().await;
    let vid = vendor_invoice(&mut db).await;
    assert_eq!(
        submit(app, &submitter.token, "vendor_invoice", vid)
            .await
            .status,
        201
    );
    let body = app
        .get(
            &outsider.token,
            &format!("/api/v1/approvals/entity/vendor_invoice/{vid}"),
        )
        .await
        .json();
    assert_eq!(body["request"], Value::Null);
    assert_eq!(body["history"], json!([]));
    assert_eq!(body["gate"], "pending");
    let body = app
        .get(
            &submitter.token,
            &format!("/api/v1/approvals/entity/vendor_invoice/{vid}"),
        )
        .await
        .json();
    assert_eq!(body["request"]["status"], "pending");
    assert_eq!(body["history"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_reject_and_approve_through_the_api() {
    let app = app();
    let sa = app.superadmin().await;
    let bdr = person(app, &["sales_bizdev"], &["approvals:view"]).await;
    let mut db = app.db().await;
    let qid = custom_quotation(&mut db, 30.0).await;
    let r = submit(app, &bdr.token, "quotation", qid).await;
    let rid = r.json()["id"].as_str().unwrap().to_string();
    // the Owner (the seeded superadmin) is awaited
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&granary_core::config::settings().SUPERADMIN_EMAIL)
        .fetch_one(&mut db)
        .await
        .unwrap();
    let owner = app.token_for(owner_id);
    let awaiting = app.get(&owner, "/api/v1/approvals").await.json();
    assert!(awaiting
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == rid.as_str()));
    let r = app
        .post(
            &owner,
            &format!("/api/v1/approvals/{rid}/reject"),
            json!({"comment": "Too large"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "rejected");
    let actions: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT action, comment FROM approval_actions WHERE request_id = $1 ORDER BY created_at",
    )
    .bind(Uuid::parse_str(&rid).unwrap())
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert!(
        actions.contains(&("rejected".to_string(), Some("Too large".to_string()))),
        "{actions:?}"
    );
    let _ = sa;
}

#[tokio::test]
async fn test_overdue_steps_are_reminded_once_a_day() {
    let app = app();
    let _g = app.serial().await;
    let bdr = person(app, &["sales_bizdev"], &["approvals:view"]).await;
    let mut db = app.db().await;
    let qid = custom_quotation(&mut db, 30.0).await;
    let r = submit(app, &bdr.token, "quotation", qid).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let rid = Uuid::parse_str(r.json()["id"].as_str().unwrap()).unwrap();
    let mut steps: Value = sqlx::query_scalar("SELECT steps FROM approval_requests WHERE id = $1")
        .bind(rid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    let seq: Option<i32> =
        sqlx::query_scalar("SELECT current_step_seq FROM approval_requests WHERE id = $1")
            .bind(rid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    for s in steps.as_array_mut().unwrap() {
        if s["seq"].as_i64() == seq.map(i64::from) && s["is_notify_only"] != true {
            s["due_at"] = json!((Utc::now() - Duration::hours(5)).to_rfc3339());
        }
    }
    sqlx::query("UPDATE approval_requests SET steps = $2 WHERE id = $1")
        .bind(rid)
        .bind(&steps)
        .execute(&mut db)
        .await
        .unwrap();
    let n = app
        .on_server(async {
            let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
                .await
                .unwrap();
            let mut conn = pool.begin().await.unwrap();
            let n = granary_services::approvals::notify::scope(
                granary_services::approvals::engine::remind_overdue(&mut conn, None),
            )
            .await
            .unwrap();
            conn.commit().await.unwrap();
            n
        })
        .await;
    assert!(n >= 1);
    let reminded: bool = sqlx::query_scalar(
        "SELECT last_reminded_at IS NOT NULL FROM approval_requests WHERE id = $1",
    )
    .bind(rid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(reminded);
    let submitter: Vec<String> =
        sqlx::query_scalar("SELECT title FROM notifications WHERE user_id = $1 AND entity_id = $2")
            .bind(bdr.id)
            .bind(qid.to_string())
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert!(
        submitter.iter().any(|t| t.starts_with("Approval overdue")),
        "{submitter:?}"
    );
}
