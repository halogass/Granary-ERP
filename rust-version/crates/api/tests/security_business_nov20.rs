//! 20 Nov 2026: security and business-rule fixes.
//!
//! - leave: an HR leave manager may delete anyone's request (the owner rule is in port_findings_3.rs)
//! - scheduling a CRM email needs crm:send_email, a WhatsApp message the WhatsApp send gate (as sending now)
//! - vendor work orders follow their lifecycle (no draft -> completed)
//! - quotation header fields longer than their columns are a 422, not a 500
//! - the public support form's team and category lists (active only, no sign-in)
//! - the quotation notes check reads a saved quotation's facts (as the approval gate does); the notes template
//!   keeps the line taxes of lines that can't be priced yet
//! - FX "Sync now" with finance:manage_fx

mod support;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

use granary_services::quotation_notes as qn;

const TAG: &str = "security_business_nov20";
const API: &str = "/api/v1";

fn app_() -> &'static TestApp {
    app(TAG)
}

// ─── leave ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_hr_leave_manager_may_delete_anyones_pending_leave() {
    let a = app_();
    let requester = a.user_with(&["hr:submit_leave"]).await;
    let hr = a.user_with(&["hr:approve_leave"]).await;
    let mut db = a.db().await;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO leave_requests (id, submitted_by_id, leave_type, start_date, end_date, days, status, \
         is_deleted) VALUES (gen_random_uuid(), $1, 'annual', '2026-12-01', '2026-12-02', 2, 'pending', false) \
         RETURNING id",
    )
    .bind(requester.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let other = a.user_with(&["hr:submit_leave"]).await;
    let r = a
        .delete(&other.token, &format!("{API}/hr/leave/{id}"))
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    let r = a.delete(&hr.token, &format!("{API}/hr/leave/{id}")).await;
    assert_eq!(r.status, 204, "{}", r.text());
}

// ─── scheduled messages ──────────────────────────────────────────────────────

fn scheduled(channel: &str) -> Value {
    json!({"channel": channel, "chat_id": "c1", "recipient": "customer@example.com", "subject": "Hello",
           "body": "Hi there", "scheduled_at": "2030-01-01T09:00:00Z"})
}

#[tokio::test]
async fn scheduling_needs_the_same_permission_as_sending_now() {
    let a = app_();
    let url = format!("{API}/crm/scheduled");
    // email: crm:send_email, as the CRM inbox reply
    let nobody = a.user_with(&["crm:view_own"]).await;
    let r = a.post(&nobody.token, &url, scheduled("email")).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"detail": "Permission required: crm:send_email"})
    );
    let mailer = a.user_with(&["crm:send_email"]).await;
    let r = a.post(&mailer.token, &url, scheduled("email")).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    // editing it later checks again
    let r = a
        .patch(&nobody.token, &format!("{url}/{id}"), json!({"body": "x"}))
        .await;
    assert_eq!(r.status, 404, "only the author's own messages");
    // WhatsApp: the immediate send gate (inbox access and crm:send_whatsapp)
    let r = a.post(&mailer.token, &url, scheduled("whatsapp")).await;
    assert_eq!(r.status, 403, "{}", r.text());
    let sender_only = a.user_with(&["crm:send_whatsapp"]).await;
    let r = a
        .post(&sender_only.token, &url, scheduled("whatsapp"))
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    let wa = a
        .user_with(&["crm:view_whatsapp", "crm:send_whatsapp"])
        .await;
    let r = a.post(&wa.token, &url, scheduled("whatsapp")).await;
    assert_eq!(r.status, 201, "{}", r.text());
}

// ─── vendor work orders ──────────────────────────────────────────────────────

async fn vendor(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, \
         total_spend, spend_currency, is_deleted) VALUES ($1, $2, 'component_supplier', 'active', false, \
         false, 0, 'JPY', false)",
    )
    .bind(id)
    .bind(u("Vendor"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn work_order(db: &mut PgConnection, vendor: Uuid, status: &str, by: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders (id, number, vendor_id, title, payment_type, currency, status, created_by_id, \
         withholding_applicable, is_deleted) VALUES ($1, $2, $3, 'Install', 'fixed', 'JPY', $4, $5, false, false)",
    )
    .bind(id)
    .bind(u("WO"))
    .bind(vendor)
    .bind(status)
    .bind(by)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_work_order_follows_its_lifecycle() {
    let a = app_();
    let sa = a.superadmin().await;
    let mut db = a.db().await;
    let v = vendor(&mut db).await;
    let status = |id: Uuid, s: &str| {
        let token = sa.token.clone();
        let s = s.to_string();
        async move {
            a.patch(
                &token,
                &format!("{API}/vendor/work-orders/{id}/status"),
                json!({ "status": s }),
            )
            .await
        }
    };
    // a draft can't skip the activation gate
    let draft = work_order(&mut db, v, "draft", sa.id).await;
    for to in ["completed", "paused"] {
        let r = status(draft, to).await;
        assert_eq!(r.status, 400, "draft -> {to}: {}", r.text());
        assert_eq!(
            r.json(),
            json!({ "detail": format!("A draft work order cannot be moved to {to}") })
        );
    }
    let s: String = sqlx::query_scalar("SELECT status FROM work_orders WHERE id = $1")
        .bind(draft)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(s, "draft");
    assert_eq!(status(draft, "cancelled").await.status, 200);
    assert_eq!(
        status(draft, "active").await.status,
        400,
        "cancelled is final"
    );
    // active work: paused, resumed through the gate, completed; completed is final
    let active = work_order(&mut db, v, "active", sa.id).await;
    assert_eq!(status(active, "paused").await.status, 200);
    let done = work_order(&mut db, v, "active", sa.id).await;
    let r = status(done, "completed").await;
    assert_eq!(r.status, 200, "{}", r.text());
    for to in ["active", "draft", "cancelled", "paused"] {
        assert_eq!(status(done, to).await.status, 400, "completed -> {to}");
    }
}

// ─── quotation header lengths ────────────────────────────────────────────────

async fn product(db: &mut PgConnection, sku: &str, prices: &[(&str, &str)]) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, category, unit, sale_model, price_metric, \
         pricing_mode, unit_equivalents, is_active, is_orderable, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, 'physical', 'hardware', 'Unit', 'one_time', 'unit', 'list', 1, true, true, false, \
         false)",
    )
    .bind(id)
    .bind(u(sku))
    .bind(format!("{sku} product"))
    .execute(&mut *db)
    .await
    .unwrap();
    for (cur, amount) in prices {
        sqlx::query(
            "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) \
             VALUES ($1, $2, $3, $4, '2020-01-01')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(*cur)
        .bind(amount.parse::<Decimal>().unwrap())
        .execute(&mut *db)
        .await
        .unwrap();
    }
    id
}

/// A customer quoted without tax (a default sales tax never disagrees with the notes).
async fn customer(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, country, type, revenue_currency, tags, is_deleted, custom, \
         quote_without_tax) VALUES ($1, $2, 'United Kingdom', '{}', 'JPY', '{}', false, '{}', true)",
    )
    .bind(id)
    .bind(u("Nov20 Customer Ltd"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn draft(a: &TestApp, token: &str, org: Uuid, pid: Uuid, extra: Value) -> String {
    let mut body = json!({"currency": "JPY", "subject": u("Laptops for the office"), "validity_days": 30,
                          "customer_org_id": org.to_string(),
                          "items": [{"product_id": pid.to_string(), "quantity": 2}]});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let r = a
        .post(token, &format!("{API}/sales/quotations"), body)
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn quotation_header_fields_longer_than_their_columns_are_refused() {
    let a = app_();
    let sa = a.superadmin().await;
    let mut db = a.db().await;
    let pid = product(&mut db, "LEN-Q", &[("JPY", "1000000")]).await;
    let org = customer(&mut db).await;
    // at the limit: saved
    let qid = draft(
        a,
        &sa.token,
        org,
        pid,
        json!({"shipping_terms": "x".repeat(50)}),
    )
    .await;
    for (field, n) in [
        ("shipping_terms", 51),
        ("subject", 501),
        ("currency", 4),
        ("discount_display", 21),
    ] {
        let body = json!({"currency": "JPY", "customer_org_id": org.to_string(),
                          "items": [{"product_id": pid.to_string(), "quantity": 1}],
                          field: "X".repeat(n)});
        let r = a
            .post(&sa.token, &format!("{API}/sales/quotations"), body.clone())
            .await;
        assert_eq!(r.status, 422, "create {field}: {}", r.text());
        assert!(
            r.text().contains(field) && r.text().contains("at most"),
            "{}",
            r.text()
        );
        let r = a
            .patch(&sa.token, &format!("{API}/sales/quotations/{qid}"), body)
            .await;
        assert_eq!(r.status, 422, "update {field}: {}", r.text());
    }
    let r = a
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "items": [{"name": "N".repeat(256), "quantity": 1, "unit_price": 10}]}),
        )
        .await;
    assert_eq!(r.status, 422, "line name: {}", r.text());
}

// ─── public support form lists ────────────────────────────────────────────────

#[tokio::test]
async fn the_public_support_form_lists_active_teams_and_their_categories() {
    let a = app_();
    let mut db = a.db().await;
    let (on, off) = (Uuid::new_v4(), Uuid::new_v4());
    let (on_name, off_name) = (u("Support Team"), u("Retired Team"));
    for (id, name, active) in [(on, &on_name, true), (off, &off_name, false)] {
        sqlx::query(
            "INSERT INTO helpdesk_teams (id, name, email, description, is_active) \
             VALUES ($1, $2, 'team-inbox@example.com', 'internal notes', $3)",
        )
        .bind(id)
        .bind(name)
        .bind(active)
        .execute(&mut db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO helpdesk_categories (id, team_id, name, description) \
             VALUES (gen_random_uuid(), $1, $2, 'internal')",
        )
        .bind(id)
        .bind(format!("{name} billing"))
        .execute(&mut db)
        .await
        .unwrap();
    }
    // the staff lists still need a sign-in
    assert_eq!(
        a.get("", &format!("{API}/helpdesk/teams")).await.status,
        401
    );
    let r = a.get("", &format!("{API}/helpdesk/public/teams")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let teams = r.json();
    let mine: Vec<&Value> = teams
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"] == on_name.as_str() || t["name"] == off_name.as_str())
        .collect();
    assert_eq!(
        mine,
        vec![&json!({"id": on.to_string(), "name": on_name})],
        "active only, id and name only"
    );
    let r = a
        .get(
            "",
            &format!("{API}/helpdesk/public/categories?team_id={on}"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let cats = r.json();
    assert_eq!(cats.as_array().unwrap().len(), 1, "{cats}");
    assert_eq!(cats[0]["name"], format!("{on_name} billing"));
    assert!(cats[0].get("description").is_none());
    let r = a
        .get(
            "",
            &format!("{API}/helpdesk/public/categories?team_id={off}"),
        )
        .await;
    assert_eq!(
        r.json(),
        json!([]),
        "an inactive team's categories are hidden"
    );
    let all = a
        .get("", &format!("{API}/helpdesk/public/categories"))
        .await
        .json();
    assert!(
        !all.to_string().contains(&off_name),
        "no inactive team's categories in the full list"
    );
}

// ─── quotation notes: the check and the template ─────────────────────────────

#[tokio::test]
async fn the_notes_check_reads_the_saved_quotation_like_the_approval_gate() {
    let a = app_();
    let _g = a.serial().await;
    let sa = a.superadmin().await;
    a.set_setting(qn::BLOCK_KEY, "true").await;
    let mut db = a.db().await;
    let pid = product(&mut db, "LAPTOP-N", &[("JPY", "1000000")]).await;
    let org = customer(&mut db).await;
    let qid = draft(
        a,
        &sa.token,
        org,
        pid,
        json!({"shipping_terms": "EXW Tokyo"}),
    )
    .await;
    let id = Uuid::parse_str(&qid).unwrap();
    // good products notes, then the subscription family (it adds Non-payment) chosen by hand on the saved quotation
    let s = qn::stored(&mut db, id).await.unwrap().unwrap();
    let set = qn::load(&mut db).await.unwrap();
    let args = qn::NotesArgs {
        deal: json!({"scope": "two laptops"})
            .as_object()
            .cloned()
            .unwrap(),
        language: "en".into(),
        ..Default::default()
    };
    let good = qn::build(&mut db, &set, &s.facts, &args)
        .await
        .unwrap()
        .unwrap()
        .html;
    sqlx::query("UPDATE quotations SET notes = $2, notes_family = 'subscription' WHERE id = $1")
        .bind(id)
        .bind(&good)
        .execute(&mut db)
        .await
        .unwrap();
    let form = json!({"currency": "JPY", "customer_org_id": org.to_string(), "validity_days": 30,
                      "subject": s.facts.subject, "notes": good,
                      "items": [{"product_id": pid.to_string(), "quantity": 2, "tax_pct": 0}]});
    let check = |body: Value| {
        let token = sa.token.clone();
        async move {
            let r = a
                .post(&token, &format!("{API}/sales/quotation-notes/check"), body)
                .await;
            assert_eq!(r.status, 200, "{}", r.text());
            r.json()
        }
    };
    // the form alone reads as products (what the old check said)
    let c = check(form.clone()).await;
    assert_eq!(c["family"], "products", "{c}");
    // the approval refuses: the saved quotation is a subscription
    let r = a
        .post(
            &sa.token,
            &format!("{API}/approvals/submit"),
            json!({"entity_type": "quotation", "entity_id": qid}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("house template"), "{}", r.text());
    // with the quotation named, the check reads the same facts and refuses too
    let mut named = form.clone();
    named["quotation_id"] = json!(qid);
    let c = check(named).await;
    assert_eq!(c["family"], "subscription", "{c}");
    assert_eq!(c["ok"], false, "{c}");
    assert!(
        c["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["blocking"] == true),
        "{c}"
    );
    // the id alone: everything from the saved quotation
    let c = check(json!({ "quotation_id": qid })).await;
    assert_eq!(c["family"], "subscription", "{c}");
    assert_eq!(c["ok"], false, "{c}");
    // an unknown quotation
    let r = a
        .post(
            &sa.token,
            &format!("{API}/sales/quotation-notes/check"),
            json!({"quotation_id": Uuid::new_v4().to_string()}),
        )
        .await;
    assert_eq!(r.status, 404, "{}", r.text());
    a.set_setting(qn::BLOCK_KEY, "false").await;
}

#[tokio::test]
async fn the_notes_template_keeps_the_tax_of_lines_that_cannot_be_priced_yet() {
    let a = app_();
    let sa = a.superadmin().await;
    let mut db = a.db().await;
    // priced in JPY only: a EUR line has no catalog price yet
    let pid = product(&mut db, "LAPTOP-X", &[("JPY", "1000000")]).await;
    let org = customer(&mut db).await;
    let items =
        json!([{"product_id": pid.to_string(), "quantity": 1, "unit_price": 7000, "tax_pct": 11}]);
    // the pricing can't price it, so the route reads the lines as they are
    let tool_items = vec![json!({"product_id": pid.to_string(), "quantity": 1,
                                 "_tax": {"tax_id": null, "tax_pct": "11"}})];
    let org_s = org.to_string();
    let priced = a
        .on_server(async move {
            let mut db = a.db().await;
            granary_services::ai::drafting::_price_lines_with(
                &mut db,
                &tool_items,
                "EUR",
                Some(&org_s),
                None,
            )
            .await
            .map(|_| ())
        })
        .await;
    assert!(
        matches!(priced, Err(granary_core::ApiError::Http { .. })),
        "{priced:?}"
    );
    let r = a
        .post(
            &sa.token,
            &format!("{API}/sales/quotation-notes/template"),
            json!({"currency": "EUR", "customer_org_id": org.to_string(), "validity_days": 30,
                   "items": items}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let html = r.json()["preview_html"].as_str().unwrap().to_string();
    assert!(
        html.contains("Prices include sales tax where shown per line"),
        "{html}"
    );
    assert!(!html.contains("Prices exclude sales tax"), "{html}");
    // every line untaxed: the export sentence
    let r = a
        .post(
            &sa.token,
            &format!("{API}/sales/quotation-notes/template"),
            json!({"currency": "EUR", "customer_org_id": org.to_string(), "validity_days": 30,
                   "items": [{"product_id": pid.to_string(), "quantity": 1, "tax_pct": 0}]}),
        )
        .await;
    let html = r.json()["preview_html"].as_str().unwrap().to_string();
    assert!(html.contains("Prices exclude sales tax"), "{html}");
}

// ─── FX sync ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fx_sync_now_takes_the_manual_rates_permission() {
    let a = app_();
    let _g = a.serial().await;
    let url = format!("{API}/system/currencies/sync-now");
    let nobody = a.user_with(&["finance:view_reports"]).await;
    let r = a.post(&nobody.token, &url, json!(null)).await;
    assert_eq!(r.status, 403, "{}", r.text());
    let fx = a.user_with(&["finance:manage_fx"]).await;
    a.enqueued();
    let r = a.post(&fx.token, &url, json!(null)).await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert!(a
        .enqueued()
        .iter()
        .any(|j| j.task == "app.tasks.sync_exchange_rates"));
}
