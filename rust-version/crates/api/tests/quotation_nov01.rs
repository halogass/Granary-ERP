//! 1 Nov 2026 (Globex session, Owner decisions 1 Nov), Stream A: a quotation takes an Owner-approved discount (a PDF
//! Discount line with its note, or notes only), is repriced to today, is withdrawn from approval to change it and
//! is cancelled, through Nary's tools and cards (the chat's Confirm: POST /nary/actions/run) and your AI client (MCP)
//! at MCP full power (nary_apply for money and cancelling), with the permission rules; the editor refuses edits
//! while the approver has it; a proposal revision dates its quotation copies today and reprices them.

mod support;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "quotation_nov01";
const API: &str = "/api/v1";

/// The person with their permissions.
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
    .bind(u("NOV-BOX"))
    .bind(u("Laptop Server 25"))
    .execute(&mut *db)
    .await
    .unwrap();
    let yesterday = today() - chrono::Duration::days(1);
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from, valid_to) \
         VALUES ($1, $2, 'JPY', $3, '2020-01-01', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(old.parse::<Decimal>().unwrap())
    .bind(yesterday)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(new.parse::<Decimal>().unwrap())
    .bind(today())
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A draft: 2 x the catalog item and a typed installation line, issued `days_ago` days ago.
async fn draft(app: &TestApp, token: &str, pid: Uuid, days_ago: i64) -> (String, String) {
    let issue = (today() - chrono::Duration::days(days_ago)).to_string();
    let r = app
        .post(
            token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "issue_date": issue, "subject": u("Globex SDK"), "validity_days": 14,
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

async fn pending(db: &mut PgConnection, qid: &str, by: Uuid) -> Uuid {
    let req = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO approval_requests (id, entity_type, entity_id, status, document_revision, submitted_by_id, \
         submitted_at, snapshot, flags, steps) \
         VALUES ($1, 'quotation', $2::uuid, 'pending', 1, $3, now(), '{}', '{}', '[]')",
    )
    .bind(req)
    .bind(qid)
    .bind(by)
    .execute(&mut *db)
    .await
    .unwrap();
    req
}

async fn req_status(db: &mut PgConnection, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM approval_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
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

// ─── discount ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_discount_pdf_line_and_notes_only_with_the_permission_rules() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1000000").await;

    // propose_quotation_draft by the Owner: 10 % with a note, a Discount line on the PDF
    let owner = auth(&mut db, sa.id).await;
    let out = granary_services::ai::tools::catalog::_h_propose_quotation(
        &mut db,
        &owner,
        json!({"currency": "JPY", "items_json": format!("[{{\"product_id\": \"{pid}\", \"quantity\": 2}}]"),
               "discount_type": "percent", "discount_value": 10,
               "discount_note": "Owner-approved launch discount for Globex"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["payload"]["discount_type"], "percentage", "{out}");
    assert_eq!(card["payload"]["discount_display"], "pdf");
    assert!(card["discount"].as_str().unwrap().contains("10% off"));
    assert_eq!(card["discount_amount"].as_f64(), Some(200_000.0), "{out}");
    // the chat card's Confirm creates it (POST /sales/quotations as the person)
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            card["payload"].clone(),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let qid = r.json()["id"].as_str().unwrap().to_string();
    let (amount, note, display): (Option<Decimal>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT discount_amount, discount_note, discount_display FROM quotations WHERE id = $1::uuid",
    )
    .bind(&qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(amount, Some(Decimal::from(200_000)));
    assert_eq!(
        note.as_deref(),
        Some("Owner-approved launch discount for Globex")
    );
    assert_eq!(display.as_deref(), Some("pdf"));
    let data = pdf_data(&mut db, &qid).await;
    let row = data["totals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["label"] == "Discount")
        .cloned()
        .unwrap();
    assert_eq!(
        row["note"], "Owner-approved launch discount for Globex",
        "{data}"
    );
    let got = app
        .get(&sa.token, &format!("{API}/sales/quotations/{qid}"))
        .await
        .json();
    assert_eq!(
        got["discount_note"],
        "Owner-approved launch discount for Globex"
    );

    // notes only: the totals stay full, the PDF notes print the note
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &owner,
        json!({"quotation_id": qid, "discount_display": "notes_only",
               "discount_note": "Includes a 10% discount approved by the Owner."}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    let text = details(card);
    assert!(
        text.contains("Discount: 10% off") && text.contains("notes only"),
        "{text}"
    );
    assert!(!text.contains('\u{2014}'), "{text}");
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (amount, total, subtotal): (Option<Decimal>, Option<Decimal>, Option<Decimal>) =
        sqlx::query_as(
            "SELECT discount_amount, total, subtotal FROM quotations WHERE id = $1::uuid",
        )
        .bind(&qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(amount.unwrap_or_default(), Decimal::ZERO);
    assert!(total.unwrap() >= subtotal.unwrap(), "totals stay full");
    let data = pdf_data(&mut db, &qid).await;
    assert!(!data["totals"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["label"] == "Discount"));
    let notes = data["sections"][0]["html"].as_str().unwrap_or("");
    assert!(
        notes.contains("Includes a 10% discount approved by the Owner."),
        "{data}"
    );

    // without the pricing permission: no card, no discount through the editor either
    let seller = app
        .user_with(&[
            "sales:create_quotation",
            "sales:view_quotations",
            "sales:edit_quotation",
            "sales:price_manual",
        ])
        .await;
    let seller_u = auth(&mut db, seller.id).await;
    let out = granary_services::ai::tools::catalog::_h_propose_quotation(
        &mut db,
        &seller_u,
        json!({"currency": "JPY", "items_json": format!("[{{\"product_id\": \"{pid}\", \"quantity\": 1}}]"),
               "discount_type": "amount", "discount_value": 50000}),
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
    let (other, _) = draft(app, &seller.token, pid, 0).await;
    let out = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &seller_u,
        json!({"quotation_id": other, "discount_type": "percent", "discount_value": 5}),
    )
    .await
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("Owner"), "{out}");
    let r = app
        .post(
            &seller.token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "discount_note": "secret deal", "discount_display": "notes_only",
                   "items": [{"product_id": pid.to_string(), "quantity": 1}]}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    // a bad display is refused
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "discount_display": "banner",
                   "items": [{"product_id": pid.to_string(), "quantity": 1}]}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
}

// ─── reprice ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_reprice_moves_catalog_lines_to_todays_price_and_keeps_typed_ones() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1250000").await;
    let (qid, number) = draft(app, &sa.token, pid, 20).await;
    let owner = auth(&mut db, sa.id).await;

    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_reprice(
        &mut db,
        &owner,
        json!({"quotation_id": number}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "quotation.reprice", "{out}");
    let text = details(card);
    for want in [
        format!(
            "Issue date: {} \u{2192} {}",
            today() - chrono::Duration::days(20),
            today()
        ),
        "Valid until:".to_string(),
        "1,000,000 \u{2192} 1,250,000 (catalog price of the new date)".to_string(),
        "Installation: 500,000 \u{2192} 500,000 (typed price, kept)".to_string(),
        "Total: JPY ".to_string(),
    ] {
        assert!(text.contains(&want), "missing {want:?} in:\n{text}");
    }
    assert!(!text.contains('\u{2014}'));
    let r = run(app, &sa.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["label"]
        .as_str()
        .unwrap()
        .starts_with("Repriced quotation"));
    let (issue, until): (chrono::NaiveDate, chrono::NaiveDate) =
        sqlx::query_as("SELECT issue_date, valid_until FROM quotations WHERE id = $1::uuid")
            .bind(&qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(issue, today());
    assert_eq!(until, today() + chrono::Duration::days(14));
    let prices: Vec<(String, Decimal)> = sqlx::query_as(
        "SELECT name, unit_price FROM quotation_items WHERE quotation_id = $1::uuid ORDER BY sort_order",
    )
    .bind(&qid)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(prices[0].1, Decimal::from(1_250_000));
    assert_eq!(prices[1], ("Installation".into(), Decimal::from(500_000)));

    // nothing left to reprice (once out of approval)
    sqlx::query("UPDATE approval_requests SET status = 'cancelled' WHERE entity_id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    let again = granary_services::nary::quotation_lifecycle::_h_propose_quotation_reprice(
        &mut db,
        &owner,
        json!({"quotation_id": qid}),
    )
    .await
    .unwrap();
    assert!(
        again["error"]
            .as_str()
            .unwrap()
            .contains("nothing to reprice"),
        "{again}"
    );

    // the quotation page's button runs the same code
    let (qid2, _) = draft(app, &sa.token, pid, 5).await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid2}/reprice"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(
        r.json()["changes"].to_string().contains("1,250,000"),
        "{}",
        r.text()
    );
}

// ─── withdraw, pending edits ──────────────────────────────────────────────────

#[tokio::test]
async fn test_withdraw_by_the_submitter_or_the_owner_and_no_edit_while_pending() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "900000", "900000").await;
    let perms = [
        "sales:create_quotation",
        "sales:view_quotations",
        "sales:price_manual",
    ];
    let submitter = app.user_with(&perms).await;
    let other = app.user_with(&perms).await;
    let (qid, number) = draft(app, &submitter.token, pid, 0).await;
    let req = pending(&mut db, &qid, submitter.id).await;

    // the editor refuses while the approver has it, with the tools' words
    let r = app
        .patch(
            &submitter.token,
            &format!("{API}/sales/quotations/{qid}"),
            json!({"currency": "JPY", "notes": "fix", "items": [{"product_id": pid.to_string(), "quantity": 1}]}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(
        r.text().contains("waiting for approval") && r.text().contains("Withdraw"),
        "{}",
        r.text()
    );
    let sub_u = auth(&mut db, submitter.id).await;
    let upd = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &sub_u,
        json!({"quotation_id": number, "notes": "x"}),
    )
    .await
    .unwrap();
    assert!(
        upd["error"]
            .as_str()
            .unwrap()
            .contains("propose_quotation_withdraw"),
        "{upd}"
    );

    // someone else can't withdraw it
    let other_u = auth(&mut db, other.id).await;
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_withdraw(
        &mut db,
        &other_u,
        json!({"quotation_id": qid}),
    )
    .await
    .unwrap();
    assert!(
        out["error"].as_str().unwrap().contains("submitted it"),
        "{out}"
    );
    let r = app
        .post(
            &other.token,
            &format!("{API}/sales/quotations/{qid}/withdraw"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());

    // the submitter: a card, confirmed in the chat
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_withdraw(
        &mut db,
        &sub_u,
        json!({"quotation_id": number, "reason": "fix the notes"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "quotation.withdraw", "{out}");
    assert!(details(card).contains("editable draft again"));
    let r = run(app, &submitter.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(req_status(&mut db, req).await, "cancelled");
    // now it can change
    let r = app
        .patch(
            &submitter.token,
            &format!("{API}/sales/quotations/{qid}"),
            json!({"currency": "JPY", "notes": "fixed", "items": [{"product_id": pid.to_string(), "quantity": 1}]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());

    // the Owner may withdraw someone else's request (the quotation page's button)
    let req2 = pending(&mut db, &qid, submitter.id).await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/withdraw"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(req_status(&mut db, req2).await, "cancelled");
    // nothing waits any more
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/withdraw"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
}

// ─── cancel ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_cancel_hides_keeps_and_never_sends() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "700000", "700000").await;
    let perms = [
        "sales:create_quotation",
        "sales:view_quotations",
        "sales:send_quotation",
        "sales:price_manual",
    ];
    let maker = app.user_with(&perms).await;
    let stranger = app.user_with(&perms).await;
    let (qid, number) = draft(app, &maker.token, pid, 3).await;
    let req = pending(&mut db, &qid, maker.id).await;

    // a reason is required; a stranger is refused
    let maker_u = auth(&mut db, maker.id).await;
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_cancel(
        &mut db,
        &maker_u,
        json!({"quotation_id": qid}),
    )
    .await
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("reason"), "{out}");
    let stranger_u = auth(&mut db, stranger.id).await;
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_cancel(
        &mut db,
        &stranger_u,
        json!({"quotation_id": qid, "reason": "stale"}),
    )
    .await
    .unwrap();
    assert!(
        out["error"].as_str().unwrap().contains("Only the person"),
        "{out}"
    );

    // its maker: a card, confirmed
    let out = granary_services::nary::quotation_lifecycle::_h_propose_quotation_cancel(
        &mut db,
        &maker_u,
        json!({"quotation_id": number, "reason": "Replaced by the Globex SDK quotation"}),
    )
    .await
    .unwrap();
    let card = &out["proposal"];
    assert_eq!(card["action"], "quotation.cancel", "{out}");
    let text = details(card);
    assert!(
        text.contains("waiting for approval") && text.contains("withdrawn"),
        "{text}"
    );
    let r = run(app, &maker.token, card).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (status, reason, at): (
        String,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT status, cancel_reason, cancelled_at FROM quotations WHERE id = $1::uuid",
    )
    .bind(&qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(status, "cancelled");
    assert_eq!(
        reason.as_deref(),
        Some("Replaced by the Globex SDK quotation")
    );
    assert!(at.is_some());
    assert_eq!(req_status(&mut db, req).await, "cancelled");

    // hidden from the list, shown when asked for
    let all = app
        .get(&sa.token, &format!("{API}/sales/quotations?limit=500"))
        .await
        .json();
    assert!(!all.to_string().contains(&qid));
    let cancelled = app
        .get(
            &sa.token,
            &format!("{API}/sales/quotations?status=cancelled&limit=500"),
        )
        .await
        .json();
    assert!(cancelled.to_string().contains(&qid));
    let listed = granary_services::ai::tools::catalog::run_tool(
        &mut db,
        &maker_u,
        "list_quotations",
        json!({"limit": 50}),
    )
    .await;
    assert!(listed["quotations"].is_array(), "{listed}");
    assert!(!listed.to_string().contains(&qid));
    let asked = granary_services::ai::tools::catalog::run_tool(
        &mut db,
        &maker_u,
        "list_quotations",
        json!({"status": "cancelled", "limit": 50}),
    )
    .await;
    assert!(asked.to_string().contains(&qid), "{asked}");
    // never sendable, never changed again
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{qid}/send"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    let upd = granary_services::nary::quotation_updates::_h_propose_quotation_update(
        &mut db,
        &maker_u,
        json!({"quotation_id": qid, "notes": "x"}),
    )
    .await
    .unwrap();
    assert!(
        upd["error"].as_str().unwrap().contains("cancelled"),
        "{upd}"
    );
    // a sent quotation is not cancelled this way
    let (sent, _) = draft(app, &maker.token, pid, 0).await;
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
        .bind(&sent)
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &maker.token,
            &format!("{API}/sales/quotations/{sent}/cancel"),
            json!({"reason": "x"}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
}

// ─── a proposal revision dates its quotation copies today ─────────────────────

#[tokio::test]
async fn test_a_proposal_revision_copies_quotations_dated_today_and_repriced() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000", "1100000").await;
    let (qid, _) = draft(app, &sa.token, pid, 30).await;
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    let doc = granary_services::documents::core_::create(
        &mut db,
        granary_services::documents::core_::CreateArgs {
            user_id: Some(sa.id),
            title: u("Globex SDK proposal"),
            quotation_ids: vec![Uuid::parse_str(&qid).unwrap()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    granary_services::nary::commit(&mut db).await.unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/documents/{}/revise", doc.id),
            json!({"reason": "Scope changed"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let changes = r.json()["quotation_changes"].clone();
    let text = changes.to_string();
    assert!(
        text.contains("Issue date:") && text.contains("1,000,000 \u{2192} 1,100,000"),
        "{changes}"
    );
    let new_doc = r.json()["id"].as_str().unwrap().to_string();
    let (status, issue, price): (String, chrono::NaiveDate, Decimal) = sqlx::query_as(
        "SELECT q.status, q.issue_date, i.unit_price FROM sales_document_quotations l \
         JOIN quotations q ON q.id = l.quotation_id \
         JOIN quotation_items i ON i.quotation_id = q.id AND i.product_id IS NOT NULL \
         WHERE l.document_id = $1::uuid",
    )
    .bind(&new_doc)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((status.as_str(), issue), ("draft", today()));
    assert_eq!(price, Decimal::from(1_100_000));
}

// ─── your AI client (MCP) at MCP full power ─────────────────────────────────────────

async fn mcp_token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("nov01"), "scopes": scopes, "password": "pytest-pass"}),
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
async fn test_full_power_parks_money_and_cancelling_for_nary_apply() {
    let app = app(TAG);
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
    let pid = product(&mut db, "1000000", "1200000").await;
    let (qid, number) = draft(app, &sa.token, pid, 10).await;

    // the tools are listed at full power
    let r = app
        .post(
            &full,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
    for t in [
        "propose_quotation_reprice",
        "propose_quotation_withdraw",
        "propose_quotation_cancel",
    ] {
        assert!(r.text().contains(t), "{t}");
    }

    // a discount is money: parked until nary_apply
    let out = call(
        app,
        &full,
        "propose_quotation_update",
        json!({"quotation_id": number, "discount_type": "amount", "discount_value": 100000,
               "discount_note": "Owner approved on 1 Nov"}),
    )
    .await;
    assert_eq!(out["approval_required"], true, "{out}");
    assert!(
        out["final_version"].as_str().unwrap().contains("Discount:"),
        "{out}"
    );
    let applied = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": out["apply_id"]}),
    )
    .await;
    assert_eq!(applied["applied"], true, "{applied}");
    let amount: Option<Decimal> =
        sqlx::query_scalar("SELECT discount_amount FROM quotations WHERE id = $1::uuid")
            .bind(&qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(amount, Some(Decimal::from(100_000)));

    // the update put it back into approval: withdraw applies at once (act), then reprice applies at once
    sqlx::query(
        "UPDATE approval_requests SET status = 'cancelled' WHERE entity_id = $1::uuid AND status = 'pending'",
    )
    .bind(&qid)
    .execute(&mut db)
    .await
    .unwrap();
    pending(&mut db, &qid, sa.id).await;
    let out = call(
        app,
        &full,
        "propose_quotation_withdraw",
        json!({"quotation_id": qid}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    let out = call(
        app,
        &full,
        "propose_quotation_reprice",
        json!({"quotation_id": qid}),
    )
    .await;
    assert_eq!(out["applied"], true, "{out}");
    let issue: chrono::NaiveDate =
        sqlx::query_scalar("SELECT issue_date FROM quotations WHERE id = $1::uuid")
            .bind(&qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(issue, today());

    // cancelling waits for nary_apply
    let out = call(
        app,
        &full,
        "propose_quotation_cancel",
        json!({"quotation_id": qid, "reason": "Superseded by the SDK offer"}),
    )
    .await;
    assert_eq!(out["approval_required"], true, "{out}");
    let still: String = sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1::uuid")
        .bind(&qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(still, "draft");
    let applied = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": out["apply_id"]}),
    )
    .await;
    assert_eq!(applied["applied"], true, "{applied}");
    let done: String = sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1::uuid")
        .bind(&qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(done, "cancelled");

    // a seller's act connection: no discount without the permission
    let seller = app
        .user_with(&[
            "sales:create_quotation",
            "sales:view_quotations",
            "sales:price_manual",
        ])
        .await;
    let act = mcp_token(app, &seller.token, &["act"]).await;
    let (other, _) = draft(app, &seller.token, pid, 0).await;
    let out = call(
        app,
        &act,
        "propose_quotation_update",
        json!({"quotation_id": other, "discount_type": "percent", "discount_value": 5}),
    )
    .await;
    assert!(out.to_string().contains("sales:apply_discount"), "{out}");
}
