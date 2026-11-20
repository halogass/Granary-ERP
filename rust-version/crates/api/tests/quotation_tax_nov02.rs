//! 2 Nov 2026 (quotation tax backlog, Owner decisions 2 Nov): the Globex quotation got VAT 11% from
//! finance.default_sales_tax although its notes say prices exclude VAT and Withholding tax, and nobody could take it off.
//!
//! - the quotation editor's "No tax" (tax_pct 0 on a line) stays through save, reload, reprice, approval, the
//!   PDF and a revision; a draft waiting for approval is refused with the withdraw message
//! - an organization marked quote_without_tax gets no default tax on quotations, previews and renewals
//! - notes that say prices exclude tax while a line is taxed (or include VAT while none is) stop the approval
//!   and the send
//! - propose_quotation_draft / propose_quotation_update take `tax` and `tax_code` (Owner or sales:apply_discount),
//!   through the chat's Confirm (POST /nary/actions/run) and your AI client (MCP) at full power (nary_apply)
//! - propose_quotation_revision: a new revision of a sent or expired quotation with a new validity
//! - 3 Nov 2026: a revision's line kept at its predecessor's price passes the discount-type precondition

mod support;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "quotation_tax_nov02";
const API: &str = "/api/v1";
const NOTE: &str = "Prices exclude sales tax and withholding tax.";

/// The company's 11% sales tax (no tax is seeded): created once, and the default for the test currency.
async fn ensure_vat(app: &TestApp) {
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO taxes (id, code, name, rate, rate_base, tax_type, is_active, applies_to_sales, applies_to_purchases, \
         is_deleted) SELECT gen_random_uuid(), 'VAT11', 'VAT 11%', 11, 'subtotal', 'vat', true, true, true, false \
         ON CONFLICT (code) DO NOTHING",
    )
    .execute(&mut db)
    .await
    .unwrap();
    // the company's own country: its customers get the default sales tax
    sqlx::query(
        "INSERT INTO tax_configs (id, name, country_code, country_name, currency_code, is_active, effective_from, \
         created_at, updated_at) SELECT gen_random_uuid(), 'Home', 'NLD', 'Netherlands', 'JPY', true, '2024-01-01', \
         now(), now() WHERE NOT EXISTS (SELECT 1 FROM tax_configs WHERE is_active = true)",
    )
    .execute(&mut db)
    .await
    .unwrap();
    app.set_setting("finance.default_sales_tax", r#"{"JPY": "VAT11"}"#)
        .await;
}

async fn auth(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .unwrap()
}

fn today() -> chrono::NaiveDate {
    // The ERP dates documents in its own timezone (setting `timezone`, seeded Asia/Tokyo), not UTC.
    granary_core::clock::today_in(granary_core::clock::parse_tz("Asia/Tokyo").unwrap())
}

/// A catalog item at `old` JPY until yesterday and `new` JPY from today.
async fn product(db: &mut PgConnection, old: &str, new: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, sale_model, price_metric, pricing_mode, \
         is_active, is_orderable, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, 'physical', 'Unit', 'one_time', 'unit', 'list', true, true, false, false)",
    )
    .bind(id)
    .bind(u("TAX-BOX"))
    .bind(u("Laptop Server"))
    .execute(&mut *db)
    .await
    .unwrap();
    let yesterday = today() - chrono::Duration::days(1);
    for (amount, from, to) in [
        (
            old,
            chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
            Some(yesterday),
        ),
        (new, today(), None),
    ] {
        sqlx::query(
            "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from, valid_to) \
             VALUES ($1, $2, 'JPY', $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(amount.parse::<Decimal>().unwrap())
        .bind(from)
        .bind(to)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    id
}

/// A customer in the company's own country (finance.default_sales_tax gives VAT11).
async fn customer(db: &mut PgConnection, without_tax: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, country, type, revenue_currency, tags, is_deleted, custom, \
         quote_without_tax) VALUES ($1, $2, 'Netherlands', '{}', 'JPY', '{}', false, '{}', $3)",
    )
    .bind(id)
    .bind(u("Sterling Products"))
    .bind(without_tax)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A draft for `org`: 2 x the catalog item and a typed installation line, issued `days_ago` days ago, with
/// `notes`. Lines carry no tax choice: the customer's default tax applies.
async fn draft(
    app: &TestApp,
    token: &str,
    pid: Uuid,
    org: Uuid,
    days_ago: i64,
    notes: &str,
) -> (String, String) {
    let issue = (today() - chrono::Duration::days(days_ago)).to_string();
    let r = app
        .post(
            token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "issue_date": issue, "subject": u("Globex Laptop"), "validity_days": 14,
                   "customer_org_id": org.to_string(), "notes": notes,
                   "items": [{"product_id": pid.to_string(), "quantity": 2},
                             {"name": "Installation", "quantity": 1, "unit_price": 500000}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    (
        r.json()["id"].as_str().unwrap().to_string(),
        r.json()["full_number"].as_str().unwrap().to_string(),
    )
}

/// (tax_pct, tax_code) of each line, in order, and the header tax and total.
async fn taxes(
    db: &mut PgConnection,
    qid: &str,
) -> (Vec<(Decimal, Option<String>)>, Decimal, Decimal) {
    let lines: Vec<(Decimal, Option<String>)> = sqlx::query_as(
        "SELECT tax_pct, tax_code FROM quotation_items WHERE quotation_id = $1::uuid ORDER BY sort_order, created_at",
    )
    .bind(qid)
    .fetch_all(&mut *db)
    .await
    .unwrap();
    let (tax, total): (Option<Decimal>, Option<Decimal>) =
        sqlx::query_as("SELECT tax_amount, total FROM quotations WHERE id = $1::uuid")
            .bind(qid)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    (lines, tax.unwrap_or_default(), total.unwrap_or_default())
}

async fn org_name(db: &mut PgConnection, id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn notes_of(db: &mut PgConnection, qid: &str) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT notes FROM quotations WHERE id = $1::uuid")
        .bind(qid)
        .fetch_one(&mut *db)
        .await
        .unwrap()
        .unwrap_or_default()
}

fn details(card: &Value) -> String {
    card["details"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| d.as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

async fn run(app: &TestApp, token: &str, card: &Value) -> support::Resp {
    app.post(
        token,
        &format!("{API}/nary/actions/run"),
        json!({"action": card["action"], "payload": card["payload"]}),
    )
    .await
}

async fn pdf_totals(db: &mut PgConnection, qid: &str) -> Value {
    use granary_services::approvals::adapters;
    use granary_services::verification::render;
    let entity = render::load(db, "quotation", qid).await.unwrap().unwrap();
    let adapters::Entity::Quotation(doc) = &entity else {
        panic!()
    };
    let company = render::company_block(db).await.unwrap();
    render::quotation(db, doc, &company).await.unwrap().1["totals"].clone()
}

/// The editor's PATCH body for the quotation as it stands, each line with "No tax" (tax_pct 0, as
/// QuotationForm sends it), and these notes.
async fn editor_body_without_tax(app: &TestApp, token: &str, qid: &str, notes: &str) -> Value {
    let q = app
        .get(token, &format!("{API}/sales/quotations/{qid}"))
        .await
        .json();
    let items: Vec<Value> = q["items"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mut line = json!({"key": it["id"], "name": it["name"], "description": it["description"],
                "quantity": it["quantity"], "is_optional": false, "sort_order": i + 1, "tax_pct": 0});
            if it["product_id"].is_string() {
                line["product_id"] = it["product_id"].clone();
            } else {
                line["unit_price"] = it["unit_price"].clone();
            }
            line
        })
        .collect();
    json!({"currency": q["currency"], "issue_date": q["issue_date"], "valid_until": q["valid_until"],
           "customer_org_id": q["customer_org_id"], "subject": q["subject"], "notes": notes,
           "procurement_type": "standard", "items": items})
}

const ZERO: Decimal = Decimal::ZERO;

// ─── decision 1: the editor ───────────────────────────────────────────────────

#[tokio::test]
async fn test_editor_no_tax_stays_through_save_reload_reprice_approval_pdf_and_revision() {
    let app = app(TAG);
    ensure_vat(app).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1000000").await;
    let org = customer(&mut db, false).await;
    let (qid, _) = draft(app, &sa.token, pid, org, 10, "Delivery in 4 weeks.").await;
    // like QUO-2026-11-00001-1: VAT 11% on every line from finance.default_sales_tax
    let (lines, tax, _) = taxes(&mut db, &qid).await;
    assert!(
        lines
            .iter()
            .all(|(p, c)| *p == Decimal::from(11) && c.as_deref() == Some("VAT11")),
        "{lines:?}"
    );
    assert!(tax > ZERO);

    // the editor: every line "No tax", the notes say so; saved
    let body = editor_body_without_tax(
        app,
        &sa.token,
        &qid,
        &format!("<p>Delivery in 4 weeks.</p><p>{NOTE}</p>"),
    )
    .await;
    let r = app
        .patch(&sa.token, &format!("{API}/sales/quotations/{qid}"), body)
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    // reload
    let got = app
        .get(&sa.token, &format!("{API}/sales/quotations/{qid}"))
        .await
        .json();
    for it in got["items"].as_array().unwrap() {
        assert_eq!(it["tax_pct"].as_f64(), Some(0.0), "{it}");
        assert!(it["tax_id"].is_null() && it["tax_code"].is_null(), "{it}");
    }
    assert_eq!(got["tax_amount"].as_f64(), Some(0.0), "{got}");
    // reprice to today (the 1 Nov logic) keeps "no tax"
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/reprice"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (lines, tax, total) = taxes(&mut db, &qid).await;
    assert!(lines.iter().all(|(p, _)| p.is_zero()), "{lines:?}");
    assert_eq!(tax, ZERO);
    assert_eq!(total, Decimal::from(2_500_000));
    // the reprice submitted it for approval: no tax there either, and the PDF has no tax row
    let req: Option<(String, Value)> = sqlx::query_as(
        "SELECT status, snapshot FROM approval_requests WHERE entity_type = 'quotation' AND entity_id = $1::uuid \
         ORDER BY submitted_at DESC LIMIT 1",
    )
    .bind(&qid)
    .fetch_optional(&mut db)
    .await
    .unwrap();
    let (status, snapshot) = req.expect("submitted for approval");
    assert!(
        matches!(status.as_str(), "pending" | "approved"),
        "{status}"
    );
    assert!(!snapshot.to_string().contains("VAT11"), "{snapshot}");
    let totals = pdf_totals(&mut db, &qid).await;
    assert!(
        !totals
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["kind"] == "tax"),
        "{totals}"
    );
    // a revision of the sent quotation copies "no tax"
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/revise"),
            json!({"reason": "New validity"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let rev = r.json()["id"].as_str().unwrap().to_string();
    let (lines, tax, _) = taxes(&mut db, &rev).await;
    assert!(lines.iter().all(|(p, _)| p.is_zero()) && tax.is_zero());
    assert!(notes_of(&mut db, &rev).await.contains(NOTE));
}

#[tokio::test]
async fn test_editor_refuses_a_draft_waiting_for_approval_with_the_withdraw_message() {
    let app = app(TAG);
    ensure_vat(app).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1000000").await;
    let org = customer(&mut db, false).await;
    let (qid, number) = draft(app, &sa.token, pid, org, 0, "").await;
    sqlx::query(
        "INSERT INTO approval_requests (id, entity_type, entity_id, status, document_revision, submitted_by_id, \
         submitted_at, snapshot, flags, steps) \
         VALUES ($1, 'quotation', $2::uuid, 'pending', 1, $3, now(), '{}', '{}', '[]')",
    )
    .bind(Uuid::new_v4())
    .bind(&qid)
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    let body = editor_body_without_tax(app, &sa.token, &qid, NOTE).await;
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}"),
            body.clone(),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(
        r.text().contains(&number) && r.text().contains("Withdraw it from approval first"),
        "{}",
        r.text()
    );
    // withdrawn, the same save goes through
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/withdraw"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app
        .patch(&sa.token, &format!("{API}/sales/quotations/{qid}"), body)
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (lines, tax, _) = taxes(&mut db, &qid).await;
    assert!(lines.iter().all(|(p, _)| p.is_zero()) && tax.is_zero());
}

// ─── decision 3: the organization flag ────────────────────────────────────────

#[tokio::test]
async fn test_a_customer_quoted_without_tax_starts_every_document_with_no_tax() {
    let app = app(TAG);
    ensure_vat(app).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1000000").await;
    let org = customer(&mut db, false).await;
    // the toggle on the organization page
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/crm/organizations/{org}"),
            json!({"quote_without_tax": true}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let got = app
        .get(&sa.token, &format!("{API}/crm/organizations/{org}"))
        .await
        .json();
    assert_eq!(got["quote_without_tax"], true, "{got}");
    // the catalog price preview (quotation and invoice editors)
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/pricing/preview"),
            json!({"currency": "JPY", "customer_org_id": org.to_string(),
                   "lines": [{"key": "a", "product_id": pid.to_string(), "quantity": 1}]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["default_tax"].is_null(), "{}", r.text());
    assert_eq!(r.json()["tax_amount"].as_f64(), Some(0.0));
    // a quotation, and a renewal / invoice default (default_tax)
    let (qid, _) = draft(app, &sa.token, pid, org, 0, "").await;
    let (lines, tax, _) = taxes(&mut db, &qid).await;
    assert!(
        lines.iter().all(|(p, _)| p.is_zero()) && tax.is_zero(),
        "{lines:?}"
    );
    assert!(
        granary_services::pricing::default_tax(&mut db, Some(&org.to_string()), Some("JPY"))
            .await
            .unwrap()
            .is_none()
    );
    // a line can still get a tax by hand
    let vat: Uuid = sqlx::query_scalar("SELECT id FROM taxes WHERE code = 'VAT11'")
        .fetch_one(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "customer_org_id": org.to_string(),
                   "items": [{"product_id": pid.to_string(), "quantity": 1, "tax_id": vat.to_string()}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let (lines, _, _) = taxes(&mut db, r.json()["id"].as_str().unwrap()).await;
    assert_eq!(lines[0].0, Decimal::from(11));
    // an invoice card (propose_invoice_draft) for the customer: no tax
    let name = org_name(&mut db, org).await;
    let inv = granary_services::ai::drafting::build_invoice_proposal(
        &mut db,
        &json!({"customer": name, "currency": "JPY",
                "items_json": format!("[{{\"product_id\": \"{pid}\", \"quantity\": 1}}]")}),
    )
    .await
    .unwrap();
    for it in inv["payload"]["items"].as_array().unwrap() {
        assert_eq!(it["tax_pct"].as_f64(), Some(0.0), "{inv}");
    }
    // The AI client sets it back through a record change (propose_record_change's write path)
    let owner = auth(&mut db, sa.id).await;
    granary_services::crm::writes::apply_change(
        &mut db,
        &owner,
        "organization",
        "update",
        &json!({"quote_without_tax": false}),
        Some(org),
        "mcp",
    )
    .await
    .unwrap();
    let flag: bool =
        sqlx::query_scalar("SELECT quote_without_tax FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(!flag);
}

// ─── decision 4: the gate ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_notes_and_taxes_that_disagree_stop_approval_and_sending() {
    let app = app(TAG);
    ensure_vat(app).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1000000").await;
    let org = customer(&mut db, false).await;
    // the Globex case: the notes say prices exclude VAT, the lines carry VAT
    let (qid, _) = draft(app, &sa.token, pid, org, 0, NOTE).await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/approvals/submit"),
            json!({"entity_type": "quotation", "entity_id": qid}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("exclude tax"), "{}", r.text());
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/send"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("exclude tax"), "{}", r.text());
    let owner = auth(&mut db, sa.id).await;
    let got = granary_services::ai::tools::catalog::run_tool(
        &mut db,
        &owner,
        "get_quotation",
        json!({"quotation_id": qid}),
    )
    .await;
    assert!(got["tax_notes_conflict"].as_str().is_some(), "{got}");
    // an approval given before the gate existed does not let it out either
    sqlx::query(
        "INSERT INTO approval_requests (id, entity_type, entity_id, status, document_revision, submitted_by_id, \
         submitted_at, snapshot, flags, steps) \
         VALUES ($1, 'quotation', $2::uuid, 'approved', 1, $3, now(), '{}', '{}', '[]')",
    )
    .bind(Uuid::new_v4())
    .bind(&qid)
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/send"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());

    // the notes say prices include VAT, no line has tax
    let none = customer(&mut db, true).await;
    let (other, _) = draft(app, &sa.token, pid, none, 0, "Prices include VAT.").await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/approvals/submit"),
            json!({"entity_type": "quotation", "entity_id": other}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("include tax"), "{}", r.text());

    // consistent: it goes
    let (fine, _) = draft(app, &sa.token, pid, none, 0, NOTE).await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/approvals/submit"),
            json!({"entity_type": "quotation", "entity_id": fine}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
}

// ─── decision 2: Nary's chat (cards confirmed in the chat) ────────────────────

#[tokio::test]
async fn test_chat_cards_set_and_clear_tax_with_the_note_and_the_permission_rule() {
    let app = app(TAG);
    ensure_vat(app).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1000000").await;
    let org = customer(&mut db, false).await;
    let owner = auth(&mut db, sa.id).await;
    let (qid, number) = draft(app, &sa.token, pid, org, 0, "Delivery in 4 weeks.").await;
    let (_, tax_before, total_before) = taxes(&mut db, &qid).await;
    assert_eq!(tax_before, Decimal::from(275_000));

    // tax none: every line, and the notes get the sentence; the card shows tax and total before and after
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": number, "tax": "none"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    let text = details(card);
    assert!(text.contains("Tax on every line: no tax"), "{text}");
    assert!(text.contains("Tax: JPY 275,000 \u{2192} JPY"), "{text}");
    assert!(
        text.contains("Total: JPY 2,775,000 \u{2192} JPY 2,500,000"),
        "{text}"
    );
    assert!(text.contains(NOTE), "{text}");
    assert!(!text.contains('\u{2014}'), "{text}");
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (lines, tax, total) = taxes(&mut db, &qid).await;
    assert!(lines.iter().all(|(p, _)| p.is_zero()), "{lines:?}");
    assert_eq!((tax, total), (ZERO, Decimal::from(2_500_000)));
    assert_ne!(total, total_before);
    let notes = notes_of(&mut db, &qid).await;
    assert!(
        notes.contains("Delivery in 4 weeks.") && notes.contains(NOTE),
        "{notes}"
    );

    // one line taxed again while the notes say exclude: the card warns before it goes to approval
    sqlx::query("UPDATE approval_requests SET status = 'cancelled' WHERE entity_id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": qid, "change_lines_json": "[{\"line\": 1, \"tax_code\": \"VAT11\"}]"}),
    )
    .await
    .unwrap();
    let text = details(&out["proposal"]);
    assert!(text.contains("can't go to approval"), "{text}");
    assert!(text.contains("tax VAT11 11%"), "{text}");

    // tax VAT11 on every line: the sentence goes
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": qid, "tax": "VAT11"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    sqlx::query("UPDATE approval_requests SET status = 'cancelled' WHERE entity_id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    let (lines, tax, _) = taxes(&mut db, &qid).await;
    assert!(
        lines.iter().all(|(p, _)| *p == Decimal::from(11)),
        "{lines:?}"
    );
    assert_eq!(tax, Decimal::from(275_000));
    assert!(!notes_of(&mut db, &qid).await.contains(NOTE));

    // a bad code is refused with the codes there are
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": qid, "tax": "VAT20"}),
    )
    .await
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("VAT11"), "{out}");

    // propose_quotation_draft: tax none for all, one line VAT11 by tax_code
    let cust = org_name(&mut db, org).await;
    let out = granary_services::ai::tools::catalog::_h_propose_quotation(
        &mut db,
        &owner,
        json!({"currency": "JPY", "customer": cust, "notes": "Delivery in 4 weeks.", "tax": "none",
               "items_json": format!(
                   "[{{\"product_id\": \"{pid}\", \"quantity\": 2}}, \
                     {{\"name\": \"Installation\", \"quantity\": 1, \"unit_price\": 500000, \"tax_code\": \"VAT11\"}}]")}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert!(card["tax"].as_str().unwrap().contains("no tax"), "{out}");
    assert_eq!(card["tax_amount"].as_f64(), Some(55_000.0), "{out}");
    assert!(
        card["payload"]["notes"].as_str().unwrap().contains(NOTE),
        "{out}"
    );
    let items = card["payload"]["items"].as_array().unwrap();
    assert_eq!(items[0]["tax_pct"].as_f64(), Some(0.0), "{out}");
    assert_eq!(items[1]["tax_pct"].as_f64(), Some(11.0), "{out}");
    // a taxed line with the exclude sentence: the card warns
    assert!(
        card["warnings"]
            .to_string()
            .contains("can't go to approval"),
        "{out}"
    );
    // the chat's Confirm creates it as shown
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            card["payload"].clone(),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let (lines, _, _) = taxes(&mut db, r.json()["id"].as_str().unwrap()).await;
    assert_eq!(lines[0].0, ZERO);
    assert_eq!(lines[1].0, Decimal::from(11));

    // without the pricing permission: refused, as a discount is
    let seller = app
        .user_with(&[
            "sales:create_quotation",
            "sales:view_quotations",
            "sales:price_manual",
        ])
        .await;
    let seller_u = auth(&mut db, seller.id).await;
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &seller_u,
        json!({"quotation_id": qid, "tax": "none"}),
    )
    .await
    .unwrap();
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("sales:apply_discount"),
        "{out}"
    );
    let out = granary_services::ai::tools::catalog::_h_propose_quotation(
        &mut db,
        &seller_u,
        json!({"currency": "JPY", "tax": "none",
               "items_json": format!("[{{\"product_id\": \"{pid}\", \"quantity\": 1}}]")}),
    )
    .await
    .unwrap();
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("sales:apply_discount"),
        "{out}"
    );
    // and the action itself refuses it (a card made by someone else, confirmed by the seller)
    let r = app
        .post(
            &seller.token,
            &format!("{API}/nary/actions/run"),
            json!({"action": "quotation.update", "payload": {"quotation_id": qid, "tax": "none"}}),
        )
        .await;
    assert_ne!(r.status, 200, "{}", r.text());
}

// ─── decision 5: a new revision ───────────────────────────────────────────────

#[tokio::test]
async fn test_revision_of_a_sent_quotation_with_a_new_validity_same_or_todays_prices() {
    let app = app(TAG);
    ensure_vat(app).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1200000").await;
    let org = customer(&mut db, true).await;
    let owner = auth(&mut db, sa.id).await;
    let (qid, number) = draft(app, &sa.token, pid, org, 20, NOTE).await;
    sqlx::query("UPDATE quotations SET status = 'expired' WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();

    // same prices, 30 days
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_revision(
        &mut db,
        &owner,
        json!({"quotation_id": number, "validity_days": 30}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    let text = details(card);
    let until = today() + chrono::Duration::days(30);
    assert!(
        text.contains(&format!("\u{2192} {until} (30 days)")),
        "{text}"
    );
    // 3 Nov 2026 (Owner): the catalog price went up since, but a kept price is no new discount: no warning
    assert!(
        text.contains(&format!("Prices: kept from {number}")),
        "{text}"
    );
    assert!(
        !text.contains("discount type") && !text.contains("reprice=true"),
        "{text}"
    );
    assert!(!text.contains('\u{2014}'), "{text}");
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(
        r.json()["result"].get("approval").is_none(),
        "it went to approval: {}",
        r.text()
    );
    let req: Option<String> = sqlx::query_scalar(
        "SELECT r.status FROM approval_requests r JOIN quotations q ON q.id = r.entity_id \
         WHERE r.entity_type = 'quotation' AND q.previous_version_id = $1::uuid",
    )
    .bind(&qid)
    .fetch_optional(&mut db)
    .await
    .unwrap();
    assert!(
        req.is_some(),
        "the same-price revision is waiting for approval"
    );
    let (old_status,): (String,) =
        sqlx::query_as("SELECT status FROM quotations WHERE id = $1::uuid")
            .bind(&qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(old_status, "superseded");
    let (rev, issue, valid, price): (Uuid, chrono::NaiveDate, chrono::NaiveDate, Decimal) =
        sqlx::query_as(
            "SELECT q.id, q.issue_date, q.valid_until, i.unit_price FROM quotations q \
         JOIN quotation_items i ON i.quotation_id = q.id AND i.product_id IS NOT NULL \
         WHERE q.previous_version_id = $1::uuid",
        )
        .bind(&qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!((issue, valid), (today(), until));
    assert_eq!(price, Decimal::from(1_000_000), "same prices");

    // repriced, until a date, by your AI client (MCP) at full power (not money: applied at once)
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1")
        .bind(rev)
        .execute(&mut db)
        .await
        .unwrap();
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let until = today() + chrono::Duration::days(21);
    let out = call(
        app,
        &full,
        "propose_quotation_revision",
        json!({"quotation_id": rev.to_string(), "valid_until": until.to_string(), "reprice": true}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    let (issue, valid, price): (chrono::NaiveDate, chrono::NaiveDate, Decimal) = sqlx::query_as(
        "SELECT q.issue_date, q.valid_until, i.unit_price FROM quotations q \
         JOIN quotation_items i ON i.quotation_id = q.id AND i.product_id IS NOT NULL \
         WHERE q.previous_version_id = $1",
    )
    .bind(rev)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((issue, valid), (today(), until));
    assert_eq!(price, Decimal::from(1_200_000), "today's price");
    // at today's catalog price it went to approval as usual
    let req: Option<String> = sqlx::query_scalar(
        "SELECT r.status FROM approval_requests r JOIN quotations q ON q.id = r.entity_id \
         WHERE r.entity_type = 'quotation' AND q.previous_version_id = $1 ORDER BY r.submitted_at DESC LIMIT 1",
    )
    .bind(rev)
    .fetch_optional(&mut db)
    .await
    .unwrap();
    assert!(req.is_some(), "it went to approval: {out}");

    // a draft, and someone else's quotation, are refused
    let (other, _) = draft(app, &sa.token, pid, org, 0, "").await;
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_revision(
        &mut db,
        &owner,
        json!({"quotation_id": other}),
    )
    .await
    .unwrap();
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("propose_quotation_update"),
        "{out}"
    );
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
        .bind(&other)
        .execute(&mut db)
        .await
        .unwrap();
    let seller = app
        .user_with(&["sales:create_quotation", "sales:view_quotations"])
        .await;
    let seller_u = auth(&mut db, seller.id).await;
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_revision(
        &mut db,
        &seller_u,
        json!({"quotation_id": other}),
    )
    .await
    .unwrap();
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("made the quotation"),
        "{out}"
    );
}

/// 3 Nov 2026 (Owner "Yes"): a revision whose line keeps exactly the price of the same line in the quotation it
/// replaces passes the "discounted lines must state a discount type" precondition although the catalog price rose
/// since (the editor's Revise, the chat's card above, your AI client (MCP) at full power); a line whose price changed,
/// or a new line, is checked as before.
#[tokio::test]
async fn test_same_price_revision_passes_the_discount_type_check_a_changed_line_does_not() {
    let app = app(TAG);
    ensure_vat(app).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    // 1,000,000 when quoted 20 days ago, 1,200,000 today
    let pid = product(&mut db, "1000000", "1200000").await;
    let org = customer(&mut db, true).await;
    let submit = |id: String| {
        let token = sa.token.clone();
        async move {
            app.post(
                &token,
                &format!("{API}/approvals/submit"),
                json!({"entity_type": "quotation", "entity_id": id}),
            )
            .await
        }
    };
    let revised = |db_qid: String| {
        let token = sa.token.clone();
        async move {
            let r = app
                .post(
                    &token,
                    &format!("{API}/sales/quotations/{db_qid}/revise"),
                    json!({"reason": "New validity"}),
                )
                .await;
            assert_eq!(r.status, 201, "{}", r.text());
            let rev = r.json()["id"].as_str().unwrap().to_string();
            // issued today, as the editor's user dates it (today's catalog price is the higher one)
            let mut db = app.db().await;
            sqlx::query("UPDATE quotations SET issue_date = $2 WHERE id = $1::uuid")
                .bind(&rev)
                .bind(today())
                .execute(&mut db)
                .await
                .unwrap();
            rev
        }
    };
    let sent = |qid: String| async move {
        let mut db = app.db().await;
        sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
            .bind(&qid)
            .execute(&mut db)
            .await
            .unwrap();
    };

    // the editor's Revise at the same prices: submitted
    let (qid, _) = draft(app, &sa.token, pid, org, 20, NOTE).await;
    sent(qid.clone()).await;
    let rev = revised(qid.clone()).await;
    let r = submit(rev.clone()).await;
    assert!(r.status < 300, "{}", r.text());

    // a line whose price changed in the revision: refused as before
    let (qid, _) = draft(app, &sa.token, pid, org, 20, NOTE).await;
    sent(qid.clone()).await;
    let rev = revised(qid.clone()).await;
    sqlx::query(
        "UPDATE quotation_items SET unit_price = 950000 WHERE quotation_id = $1::uuid AND product_id IS NOT NULL",
    )
    .bind(&rev)
    .execute(&mut db)
    .await
    .unwrap();
    let r = submit(rev.clone()).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("discount type"), "{}", r.text());

    // a new line (another product at its old price): refused as before
    let other = product(&mut db, "1000000", "1200000").await;
    let (qid, _) = draft(app, &sa.token, pid, org, 20, NOTE).await;
    sent(qid.clone()).await;
    let rev = revised(qid.clone()).await;
    sqlx::query(
        "UPDATE quotation_items SET product_id = $2 WHERE quotation_id = $1::uuid AND product_id IS NOT NULL",
    )
    .bind(&rev)
    .bind(other)
    .execute(&mut db)
    .await
    .unwrap();
    let r = submit(rev.clone()).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("discount type"), "{}", r.text());

    // your AI client (MCP) at full power, the same prices: made and waiting for approval
    let (qid, number) = draft(app, &sa.token, pid, org, 20, NOTE).await;
    sent(qid.clone()).await;
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let out = call(
        app,
        &full,
        "propose_quotation_revision",
        json!({"quotation_id": number, "validity_days": 30}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    assert!(!out.to_string().contains("discount type"), "{out}");
    let req: Option<String> = sqlx::query_scalar(
        "SELECT r.status FROM approval_requests r JOIN quotations q ON q.id = r.entity_id \
         WHERE r.entity_type = 'quotation' AND q.previous_version_id = $1::uuid",
    )
    .bind(&qid)
    .fetch_optional(&mut db)
    .await
    .unwrap();
    assert!(req.is_some(), "it went to approval: {out}");
}

// ─── your AI client (MCP) at MCP full power: a tax change waits for nary_apply ──────

async fn mcp_token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("nov02"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> Value {
    let r = app
        .post(
            h,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": name, "arguments": args}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.json()["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }))
}

#[tokio::test]
async fn test_full_power_parks_a_tax_change_for_nary_apply() {
    let app = app(TAG);
    ensure_vat(app).await;
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let pid = product(&mut db, "1000000", "1000000").await;
    let org = customer(&mut db, false).await;
    let (qid, number) = draft(app, &sa.token, pid, org, 0, "").await;

    let r = app
        .post(
            &full,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
    assert!(r.text().contains("propose_quotation_revision"));

    // a line's tax cleared: money, parked
    let out = call(
        app,
        &full,
        "propose_quotation_update",
        json!({"quotation_id": number, "change_lines_json": "[{\"line\": 2, \"tax_code\": null}]"}),
    )
    .await;
    assert_eq!(out["approval_required"], true, "{out}");
    assert!(
        out["final_version"].as_str().unwrap().contains("Tax:"),
        "{out}"
    );
    let (lines, _, _) = taxes(&mut db, &qid).await;
    assert!(
        lines.iter().all(|(p, _)| !p.is_zero()),
        "nothing changed yet"
    );
    let applied = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": out["apply_id"]}),
    )
    .await;
    assert_eq!(applied["applied"], true, "{applied}");
    let (lines, _, _) = taxes(&mut db, &qid).await;
    assert_eq!(lines[0].0, Decimal::from(11));
    assert_eq!(lines[1].0, ZERO);

    // a draft with tax none: parked too
    let out = call(
        app,
        &full,
        "propose_quotation_draft",
        json!({"currency": "JPY", "customer": org_name(&mut db, org).await, "tax": "none",
               "items_json": format!("[{{\"product_id\": \"{pid}\", \"quantity\": 1}}]")}),
    )
    .await;
    assert_eq!(out["approval_required"], true, "{out}");
    assert!(
        out["final_version"]
            .as_str()
            .unwrap()
            .contains("tax: no tax"),
        "{out}"
    );
}
