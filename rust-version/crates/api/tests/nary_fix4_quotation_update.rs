//! Nary fix batch 4, agent D3, item 2: propose_quotation_update changes a saved DRAFT quotation from the chat.
//!
//! Case: QUO-2026-10-00001-1 (Initech Digital Ltd) was saved before its deal and contact. The card
//! links the deal (its customer and contact fill the empty ones), changes lines priced by the pricing service,
//! shows before and after, and applies through POST /nary/actions/run (what the chat card's Confirm calls),
//! which runs the quotation editor's own code. Only a draft changes; the permission is sales:create_quotation.

mod support;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "nary_fix4_quotation_update";

async fn auth(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_core::auth::load_active_user(db, id)
        .await
        .unwrap()
        .unwrap()
}

async fn propose(db: &mut PgConnection, user_id: Uuid, args: Value) -> Value {
    let user = auth(db, user_id).await;
    granary_services::nary::quotation_updates::_h_propose_quotation_update(db, &user, args)
        .await
        .unwrap()
}

/// A catalog item with an JPY list price.
async fn product(db: &mut PgConnection, price: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, sale_model, price_metric, pricing_mode, \
         is_active, is_orderable, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, 'physical', 'Unit', 'one_time', 'unit', 'list', true, true, false, false)",
    )
    .bind(id)
    .bind(u("PYT-BOX"))
    .bind(u("Laptop Server 25"))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', $3, '2020-01-01')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(price.parse::<Decimal>().unwrap())
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// Initech with Adam and a deal for both.
async fn customer(db: &mut PgConnection) -> (Uuid, Uuid, Uuid) {
    let (org, contact, deal) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(org)
    .bind(u("Initech Digital Ltd"))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, organization_id, custom, is_deleted) \
         VALUES ($1, 'Adam', $2, $3, '{}', false)",
    )
    .bind(contact)
    .bind(u("Prince"))
    .bind(org)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, organization_id, contact_id, is_deleted, custom) \
         VALUES ($1, $2, 'proposal', $3, $4, false, '{}')",
    )
    .bind(deal)
    .bind(u("Initech Laptop rollout"))
    .bind(org)
    .bind(contact)
    .execute(&mut *db)
    .await
    .unwrap();
    (org, contact, deal)
}

/// A draft quotation with no customer and no deal: 2 x the catalog item and an installation line.
async fn draft(app: &support::TestApp, token: &str, pid: Uuid) -> (String, String) {
    let today = chrono::Utc::now().date_naive().to_string();
    let r = app
        .post(
            token,
            "/api/v1/sales/quotations",
            json!({"currency": "JPY", "issue_date": today, "subject": "Laptop for Initech", "validity_days": 14,
                   "items": [{"product_id": pid.to_string(), "quantity": 2},
                             {"name": "Installation", "quantity": 1, "unit_price": 500000}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let number = r.json()["full_number"].as_str().unwrap().to_string();
    (id, number)
}

fn details(card: &Value) -> String {
    card["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn test_update_links_deal_and_contact_changes_lines_and_applies() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "1000000").await;
    let (org, contact, deal) = customer(&mut db).await;
    let (qid, number) = draft(app, &sa.token, pid).await;

    let out = propose(
        &mut db,
        sa.id,
        json!({"quotation_id": number, "deal_id": deal.to_string(), "validity_days": 30,
               "change_lines_json": "[{\"line\": 1, \"quantity\": 3}]",
               "items_json": "[{\"name\": \"On-site training\", \"quantity\": 1, \"unit_price\": 250000}]",
               "remove_lines": "2"}),
    )
    .await;
    let card = &out["proposal"];
    assert_eq!(card["kind"], "nary_action", "{out}");
    assert_eq!(card["action"], "quotation.update");
    assert_eq!(card["summary"], format!("Update quotation {number}"));
    let text = details(card);
    for want in [
        "Deal: none \u{2192} Initech Laptop rollout",
        "Customer: none \u{2192} Initech Digital Ltd",
        "Contact: none \u{2192} Adam Prince",
        "Validity: 14 days",
        "\u{2192} 30 days",
        "Lines now (JPY):",
        "2 \u{00d7} ",
        "1 \u{00d7} Installation @ 500,000 = 500,000",
        "Lines after (JPY):",
        "3 \u{00d7} ",
        "1 \u{00d7} On-site training @ 250,000 = 250,000",
        "Note: \u{201c}On-site training\u{201d} is not in the catalog",
        "Total: JPY ",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(!text.contains('\u{2014}'), "{text}");
    let payload = &card["payload"];
    assert_eq!(payload["quotation_id"], qid);
    assert_eq!(payload["remove_item_ids"].as_array().unwrap().len(), 1);

    // nothing changed yet
    let deal_now: Option<Uuid> =
        sqlx::query_scalar("SELECT deal_id FROM quotations WHERE id = $1::uuid")
            .bind(&qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(deal_now, None);

    // Confirm (the web card calls this as the user)
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/actions/run",
            json!({"action": card["action"], "payload": payload}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json()["label"],
        format!("Updated quotation {number}"),
        "{}",
        r.text()
    );

    let (d, o, c, validity, total): (Option<Uuid>, Option<Uuid>, Option<Uuid>, Option<i32>, Option<Decimal>) =
        sqlx::query_as(
            "SELECT deal_id, customer_org_id, customer_contact_id, validity_days, total FROM quotations WHERE id = $1::uuid",
        )
        .bind(&qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(
        (d, o, c, validity),
        (Some(deal), Some(org), Some(contact), Some(30))
    );
    let shown = card["total"].as_f64().unwrap();
    assert_eq!(total.unwrap().to_string().parse::<f64>().unwrap(), shown);
    let lines: Vec<(String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT name, quantity, unit_price FROM quotation_items WHERE quotation_id = $1::uuid ORDER BY sort_order",
    )
    .bind(&qid)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0].1, Decimal::from(3));
    assert_eq!(lines[0].2, Decimal::from(1_000_000));
    assert_eq!(lines[1].0, "On-site training");
    assert_eq!(lines[1].2, Decimal::from(250_000));
}

#[tokio::test]
async fn test_only_a_draft_changes_and_only_with_the_permission() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "2000000").await;
    let (qid, number) = draft(app, &sa.token, pid).await;

    // nothing to change
    let out = propose(&mut db, sa.id, json!({"quotation_id": qid})).await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("Nothing would change"),
        "{out}"
    );

    // a stale card is refused when it is confirmed
    let out = propose(
        &mut db,
        sa.id,
        json!({"quotation_id": qid, "subject": "Laptop, revised"}),
    )
    .await;
    let payload = out["proposal"]["payload"].clone();
    assert!(
        details(&out["proposal"]).contains("Subject: Laptop for Initech \u{2192} Laptop, revised"),
        "{out}"
    );
    sqlx::query(
        "UPDATE quotations SET updated_at = now() + interval '1 second' WHERE id = $1::uuid",
    )
    .bind(&qid)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/actions/run",
            json!({"action": "quotation.update", "payload": payload}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(
        r.text().contains("was changed after this card was made"),
        "{}",
        r.text()
    );

    // without sales:create_quotation: no card, and the action is refused
    let viewer = app.user_with(&["sales:view_quotations"]).await;
    let out = propose(
        &mut db,
        viewer.id,
        json!({"quotation_id": qid, "subject": "x"}),
    )
    .await;
    assert!(
        out["error"].as_str().unwrap().contains("permission"),
        "{out}"
    );
    let fresh = propose(
        &mut db,
        sa.id,
        json!({"quotation_id": qid, "subject": "Laptop, revised"}),
    )
    .await;
    let r = app
        .post(
            &viewer.token,
            "/api/v1/nary/actions/run",
            json!({"action": "quotation.update", "payload": fresh["proposal"]["payload"]}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());

    // waiting for approval, then sent: not a draft any more
    let req = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO approval_requests (id, entity_type, entity_id, status, document_revision, submitted_by_id, \
         submitted_at, snapshot, flags, steps) \
         VALUES ($1, 'quotation', $2::uuid, 'pending', 1, $3, now(), '{}', '{}', '[]')",
    )
    .bind(req)
    .bind(&qid)
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    let out = propose(
        &mut db,
        sa.id,
        json!({"quotation_id": number, "subject": "x"}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("waiting for approval"),
        "{out}"
    );
    sqlx::query("UPDATE approval_requests SET status = 'rejected' WHERE id = $1")
        .bind(req)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    let out = propose(
        &mut db,
        sa.id,
        json!({"quotation_id": number, "subject": "x"}),
    )
    .await;
    let e = out["error"].as_str().unwrap();
    assert!(
        e.contains("is sent, not a draft") && e.contains("revision"),
        "{out}"
    );
}
