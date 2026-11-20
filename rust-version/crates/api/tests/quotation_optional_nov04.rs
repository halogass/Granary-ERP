//! 4 Nov 2026 (Nary ticket, Owner decisions 4 Nov): on QUO-2026-11-00002-1 a line created with is_optional
//! counted in the subtotal, tax and total (JPY 249.500,000 instead of JPY 218.300,000) and printed as a normal line.
//!
//! - optional lines are out of the subtotal, header discount, tax and total; the route, get_quotation and the
//!   cards also give optional_subtotal / optional_tax / optional_total / total_with_optional
//! - the PDF lists them in their own section under the totals (only when there are any)
//! - an invoice from the quotation skips them
//! - a sent quotation priced before keeps its stored totals (and prints as sent); a revision or a draft's next
//!   save recomputes

mod support;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "quotation_optional_nov04";
const API: &str = "/api/v1";

async fn auth(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .unwrap()
}

async fn customer(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, country, type, revenue_currency, tags, is_deleted, custom) \
         VALUES ($1, $2, 'Japan', '{}', 'JPY', '{}', false, '{}')",
    )
    .bind(id)
    .bind(u("Contoso Customer Ltd"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// The QUO-2026-11-00002-1 shape: a laptop set (two lines, 218,300,000) and an optional dashboard line
/// (31,200,000), no tax. `optional` false makes the dashboard a normal line.
async fn laptop_quote(app: &TestApp, token: &str, org: Uuid, optional: bool) -> (String, String) {
    let r = app
        .post(
            token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "subject": u("Laptop set"), "validity_days": 14,
                   "customer_org_id": org.to_string(),
                   "items": [{"name": "Laptop set", "quantity": 1, "unit_price": 150_000_000, "tax_pct": 0},
                             {"name": "Installation and commissioning", "quantity": 1, "unit_price": 68_300_000,
                              "tax_pct": 0},
                             {"name": "Analytics dashboard", "description": "Web dashboard, 12 months",
                              "quantity": 1, "unit_price": 31_200_000, "tax_pct": 0, "is_optional": optional}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    (
        r.json()["id"].as_str().unwrap().to_string(),
        r.json()["full_number"].as_str().unwrap().to_string(),
    )
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

async fn pdf_data(db: &mut PgConnection, qid: &str) -> Value {
    use granary_services::approvals::adapters;
    use granary_services::verification::render;
    let entity = render::load(db, "quotation", qid).await.unwrap().unwrap();
    let adapters::Entity::Quotation(doc) = &entity else {
        panic!()
    };
    let company = render::company_block(db).await.unwrap();
    render::quotation(db, doc, &company).await.unwrap().1
}

async fn jpy(db: &mut PgConnection, n: i64) -> String {
    let places = granary_core::settings_store::currency_places(db, Some("JPY"))
        .await
        .unwrap();
    granary_services::verification::render::fmt_money(Some(Decimal::from(n)), Some("JPY"), places)
}

fn row_titles(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r[0]["title"].as_str().unwrap().to_string())
        .collect()
}

async fn set_status(db: &mut PgConnection, qid: &str, status: &str) {
    sqlx::query("UPDATE quotations SET status = $2 WHERE id = $1::uuid")
        .bind(qid)
        .bind(status)
        .execute(&mut *db)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_optional_line_out_of_totals_route_tool_pdf_and_invoice() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let owner = auth(&mut db, sa.id).await;
    let org = customer(&mut db).await;
    let (qid, number) = laptop_quote(app, &sa.token, org, true).await;

    // the route: 218,300,000, and the optional figures beside it
    let q = app
        .get(&sa.token, &format!("{API}/sales/quotations/{qid}"))
        .await
        .json();
    assert_eq!(
        (f(&q["subtotal"]), f(&q["tax_amount"]), f(&q["total"])),
        (218_300_000.0, 0.0, 218_300_000.0),
        "{q}"
    );
    assert_eq!(
        (
            f(&q["optional_subtotal"]),
            f(&q["optional_tax"]),
            f(&q["optional_total"]),
            f(&q["total_with_optional"])
        ),
        (31_200_000.0, 0.0, 31_200_000.0, 249_500_000.0)
    );
    assert_eq!(q["optional_in_total"], false);
    let dash = q["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "Analytics dashboard")
        .unwrap();
    assert!(dash["is_optional"] == true && f(&dash["subtotal"]) == 31_200_000.0);

    // get_quotation (the AI client and the chat)
    let out = granary_services::ai::tools::catalog::run_tool(
        &mut db,
        &owner,
        "get_quotation",
        json!({"number": number}),
    )
    .await;
    let t = &out["totals"];
    assert_eq!(f(&t["total"]), 218_300_000.0, "{out}");
    assert_eq!(f(&t["optional_subtotal"]), 31_200_000.0, "{out}");
    assert_eq!(f(&t["total_with_optional"]), 249_500_000.0, "{out}");
    assert_eq!(t["optional_in_total"], false);

    // the PDF: two lines in the table, the dashboard in its own section under the totals
    let data = pdf_data(&mut db, &qid).await;
    assert_eq!(
        row_titles(&data["rows"]),
        ["Laptop set", "Installation and commissioning"]
    );
    let total = data["totals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "total")
        .unwrap()
        .clone();
    assert_eq!(total["value"], jpy(&mut db, 218_300_000).await);
    let opt = &data["optional_items"];
    assert_eq!(opt["title"], "Optional items (not included in the total)");
    assert_eq!(row_titles(&opt["rows"]), ["Analytics dashboard"]);
    assert_eq!(opt["rows"][0][0]["sub"], "Web dashboard, 12 months");
    assert_eq!(opt["columns"].as_array().unwrap().len(), 4);
    assert_eq!(opt["totals"][0]["value"], jpy(&mut db, 31_200_000).await);
    assert_eq!(opt["note"], "Not included in the total above.");

    // a quotation without optional lines has no section (the template then prints nothing)
    let (plain, _) = laptop_quote(app, &sa.token, org, false).await;
    let data = pdf_data(&mut db, &plain).await;
    assert!(data.get("optional_items").is_none(), "{data}");
    assert_eq!(data["rows"].as_array().unwrap().len(), 3);

    // an invoice from the quotation bills the laptop set only
    set_status(&mut db, &qid, "sent").await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/generate-invoice"),
            json!({"mode": "full"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let inv = r.json()["id"].as_str().unwrap().to_string();
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM invoice_items WHERE invoice_id = $1::uuid ORDER BY sort_order",
    )
    .bind(&inv)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(names, ["Laptop set", "Installation and commissioning"]);
    let total: Decimal = sqlx::query_scalar("SELECT total FROM invoices WHERE id = $1::uuid")
        .bind(&inv)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(total, Decimal::from(218_300_000));
}

#[tokio::test]
async fn test_a_quotation_priced_before_keeps_its_totals_and_a_revision_or_save_recomputes() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let org = customer(&mut db).await;
    let (qid, _) = laptop_quote(app, &sa.token, org, true).await;
    // as stored before 4 Nov: the optional line counted
    let legacy = |id: String| {
        format!(
            "UPDATE quotations SET subtotal = 249500000, total = 249500000, rounded_total = 249500000 \
             WHERE id = '{id}'::uuid"
        )
    };
    sqlx::query(&legacy(qid.clone()))
        .execute(&mut db)
        .await
        .unwrap();
    set_status(&mut db, &qid, "sent").await;

    // a sent one keeps its stored total, says so, and prints as it was sent
    let q = app
        .get(&sa.token, &format!("{API}/sales/quotations/{qid}"))
        .await
        .json();
    assert_eq!(f(&q["total"]), 249_500_000.0);
    assert_eq!(q["optional_in_total"], true);
    assert_eq!(f(&q["total_with_optional"]), 249_500_000.0);
    let data = pdf_data(&mut db, &qid).await;
    assert_eq!(data["rows"].as_array().unwrap().len(), 3);
    assert!(data.get("optional_items").is_none(), "{data}");

    // a revision works its totals out again from the copied lines
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/revise"),
            json!({"reason": "Optional dashboard out of the total"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let rev = r.json()["id"].as_str().unwrap().to_string();
    let q = app
        .get(&sa.token, &format!("{API}/sales/quotations/{rev}"))
        .await
        .json();
    assert_eq!(
        (f(&q["subtotal"]), f(&q["total"]), f(&q["rounded_total"])),
        (218_300_000.0, 218_300_000.0, 218_300_000.0),
        "{q}"
    );
    assert_eq!(q["optional_in_total"], false);
    assert_eq!(f(&q["total_with_optional"]), 249_500_000.0);
    let data = pdf_data(&mut db, &rev).await;
    assert_eq!(data["rows"].as_array().unwrap().len(), 2);
    assert_eq!(
        row_titles(&data["optional_items"]["rows"]),
        ["Analytics dashboard"]
    );

    // a draft priced before is totalled the new way on its next reprice (or save)
    let (draft, _) = laptop_quote(app, &sa.token, org, true).await;
    sqlx::query(&legacy(draft.clone()))
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{draft}/reprice"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let total: Option<Decimal> =
        sqlx::query_scalar("SELECT total FROM quotations WHERE id = $1::uuid")
            .bind(&draft)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(total, Some(Decimal::from(218_300_000)));
}

#[tokio::test]
async fn test_cards_show_the_optional_section_and_both_totals() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let owner = auth(&mut db, sa.id).await;
    let org = customer(&mut db).await;
    let cust: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(org)
        .fetch_one(&mut db)
        .await
        .unwrap();

    // propose_quotation_draft: the total without the optional line, and its figures
    let out = granary_services::ai::tools::catalog::_h_propose_quotation(
        &mut db,
        &owner,
        json!({"currency": "JPY", "customer": cust, "tax": "none",
               "items_json": "[{\"name\": \"Laptop set\", \"quantity\": 1, \"unit_price\": 150000000}, \
                               {\"name\": \"Installation\", \"quantity\": 1, \"unit_price\": 68300000}, \
                               {\"name\": \"Analytics dashboard\", \"quantity\": 1, \"unit_price\": 31200000, \
                                \"is_optional\": true}]"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(f(&card["total"]), 218_300_000.0, "{out}");
    assert_eq!(f(&card["optional_subtotal"]), 31_200_000.0, "{out}");
    assert_eq!(f(&card["total_with_optional"]), 249_500_000.0, "{out}");

    // Nary's quotation card: the billed lines, then the optional ones apart, then both totals
    let price = json!({"currency": "JPY", "total": "218300000", "tax_amount": "0", "subtotal": "218300000",
        "optional_subtotal": "31200000", "optional_tax": "0", "optional_total": "31200000",
        "total_with_optional": "249500000", "lines": [
            {"name": "Laptop set", "quantity": "1", "unit_price": "150000000", "amount": "150000000",
             "sale_model": "one_time", "discount_pct": "0", "is_optional": false},
            {"name": "Analytics dashboard", "quantity": "1", "unit_price": "31200000", "amount": "31200000",
             "sale_model": "one_time", "discount_pct": "0", "is_optional": true},
            {"name": "Installation", "quantity": "1", "unit_price": "68300000", "amount": "68300000",
             "discount_pct": "0", "is_optional": false}]});
    let payload = json!({"subject": "Laptop set", "currency": "JPY", "items": []});
    let text = granary_services::nary::cards::render(
        &mut db,
        granary_services::nary::cards::CardView {
            action: "quotation.create_draft",
            payload: &payload,
            diff: &json!({"label": "Laptop set"}),
            price: &price,
            rationale: None,
        },
    )
    .await
    .unwrap();
    let at = |s: &str| {
        text.find(s)
            .unwrap_or_else(|| panic!("{s:?} missing from:\n{text}"))
    };
    assert!(at("• 1 × Installation") < at("Optional items (not included in the total):"));
    assert!(at("Optional items (not included in the total):") < at("• 1 × Analytics dashboard"));
    assert!(at("• 1 × Analytics dashboard") < at("Total: JPY 218,300,000"));
    assert!(
        text.contains("Optional items: JPY 31,200,000; total with optional items: JPY 249,500,000"),
        "{text}"
    );

    // propose_quotation_update: the card says what is optional and both totals
    let (qid, _) = laptop_quote(app, &sa.token, org, true).await;
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": qid, "subject": "Laptop set, with options"}),
    )
    .await
    .unwrap();
    let details: Vec<String> = out["proposal"]["details"]
        .as_array()
        .unwrap_or_else(|| panic!("{out}"))
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect();
    let text = details.join("\n");
    assert!(
        text.contains("Total: JPY 218,300,000 (unchanged)"),
        "{text}"
    );
    assert!(
        text.contains(
            "Optional items (not in the total): JPY 31,200,000; total with optional items: JPY 249,500,000"
        ),
        "{text}"
    );
}
