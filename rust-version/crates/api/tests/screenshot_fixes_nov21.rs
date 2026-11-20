//! 21 Nov 2026, found while capturing the documentation screenshots:
//! - the invoice page's Client card said "No client set." on invoices with a customer (the detail served only
//!   customer_org_id / customer_contact_id; the page reads customer_org / customer_contact);
//! - the quotation list's Client column showed "-" on every row (same: no customer_org / customer_contact);
//! - an agent's (MCP) approval card held the whole proposal as `after.proposal`, shown as raw JSON;
//! - the kickoff card (handoff.kickoff) held the raw handoff package with its nulls.

mod support;

use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "screenshot_fixes";

/// An organization with an email and a contact in it: (org id, org name, contact id, contact last name).
async fn customer(db: &mut PgConnection) -> (Uuid, String, Uuid, String) {
    let org_id = Uuid::new_v4();
    let org = u("Aozora Accounting");
    sqlx::query(
        "INSERT INTO organizations (id, name, email, type, revenue_currency, tags, is_deleted, custom) \
         VALUES ($1, $2, 'billing@aozora.example.com', '{customer}', 'JPY', '{}', false, '{}')",
    )
    .bind(org_id)
    .bind(&org)
    .execute(&mut *db)
    .await
    .unwrap();
    let contact_id = Uuid::new_v4();
    let last = u("Fujii").replace(' ', "");
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, email, organization_id, is_primary, tags, is_deleted, custom) \
         VALUES ($1, 'Sora', $2, 'sora@aozora.example.com', $3, true, '{}', false, '{}')",
    )
    .bind(contact_id)
    .bind(&last)
    .bind(org_id)
    .execute(&mut *db)
    .await
    .unwrap();
    (org_id, org, contact_id, last)
}

#[tokio::test]
async fn the_invoice_detail_names_its_organization_and_contact() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (org_id, org, contact_id, last) = customer(&mut db).await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/finance/invoices",
            json!({"invoice_type": "proforma", "currency": "JPY", "customer_org_id": org_id.to_string(),
                   "customer_contact_id": contact_id.to_string(), "customer_name": org,
                   "items": [{"name": "Colour MFP", "quantity": 1, "unit_price": 100000}]}),
        )
        .await;
    assert!(r.status == 200 || r.status == 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let inv = app
        .get(&sa.token, &format!("/api/v1/finance/invoices/{id}"))
        .await
        .json();
    assert_eq!(
        inv["customer_org"],
        json!({"id": org_id.to_string(), "name": org, "email": "billing@aozora.example.com"}),
        "{inv}"
    );
    assert_eq!(
        inv["customer_contact"],
        json!({"id": contact_id.to_string(), "full_name": format!("Sora {last}"), "email": "sora@aozora.example.com"}),
        "{inv}"
    );

    // without a customer both are null (the page then says "No client set.")
    let r = app
        .post(
            &sa.token,
            "/api/v1/finance/invoices",
            json!({"invoice_type": "proforma", "currency": "JPY",
                   "items": [{"name": "Toner", "quantity": 1, "unit_price": 1000}]}),
        )
        .await;
    let id = r.json()["id"].as_str().unwrap().to_string();
    let inv = app
        .get(&sa.token, &format!("/api/v1/finance/invoices/{id}"))
        .await
        .json();
    assert!(
        inv["customer_org"].is_null() && inv["customer_contact"].is_null(),
        "{inv}"
    );
}

#[tokio::test]
async fn the_quotation_list_names_each_rows_client() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (org_id, org, contact_id, last) = customer(&mut db).await;
    let subject = u("Printer fleet");
    let r = app
        .post(
            &sa.token,
            "/api/v1/sales/quotations",
            json!({"currency": "JPY", "subject": subject, "customer_org_id": org_id.to_string(),
                   "customer_contact_id": contact_id.to_string(),
                   "items": [{"name": "Colour MFP", "quantity": 2, "unit_price": 100000}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let qid = r.json()["id"].as_str().unwrap().to_string();
    let list = app
        .get(
            &sa.token,
            &format!(
                "/api/v1/sales/quotations?q={}&limit=50",
                subject.replace(' ', "%20")
            ),
        )
        .await;
    assert_eq!(list.status, 200, "{}", list.text());
    let rows = list.json();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["id"] == json!(qid))
        .unwrap_or_else(|| panic!("{rows}"))
        .clone();
    assert_eq!(
        row["customer_org"],
        json!({"id": org_id.to_string(), "name": org}),
        "{row}"
    );
    assert_eq!(
        row["customer_contact"],
        json!({"id": contact_id.to_string(), "full_name": format!("Sora {last}")}),
        "{row}"
    );
}

/// `describe` of a registered Nary action, as executor.propose runs it.
async fn describe(db: &mut PgConnection, action: &str, payload: Value) -> Value {
    let a = granary_services::nary::actions::get(action).unwrap();
    let clean = a.validate(&payload).unwrap();
    let d = a.describe.clone().expect("the action describes its card");
    d(db, None, &clean).await.unwrap()
}

fn no_nested_proposal(diff: &Value) {
    assert!(diff["after"].get("proposal").is_none(), "{diff}");
}

#[tokio::test]
async fn an_agents_card_shows_what_nary_s_own_card_of_that_kind_shows() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (org_id, org, contact_id, last) = customer(&mut db).await;

    // a CRM change: the same before/after as crm.record_change
    let change = json!({"kind": "record_change", "object_type": "organization", "action": "update",
                        "record_id": org_id.to_string(), "after": {"city": "Osaka"}, "summary": "Move to Osaka"});
    let diff = describe(
        &mut db,
        "mcp.proposal",
        json!({"proposal": change, "client": "Desktop AI client", "proposer_id": sa.id.to_string()}),
    )
    .await;
    let own = describe(
        &mut db,
        "crm.record_change",
        json!({"object_type": "organization", "action": "update", "record_id": org_id.to_string(),
               "fields": {"city": "Osaka"}}),
    )
    .await;
    no_nested_proposal(&diff);
    assert_eq!(diff["after"], own["after"], "{diff}");
    // follow-up: money fields are formatted in the record's currency; the card has a title of its kind
    assert_eq!(own["currency"], "JPY", "{own}");
    assert_eq!(diff["currency"], "JPY", "{diff}");
    assert_eq!(diff["title"], "Update organization", "{diff}");
    assert_eq!(diff["before"], own["before"], "{diff}");
    assert!(
        diff["label"]
            .as_str()
            .unwrap()
            .contains("(by Desktop AI client for"),
        "{diff}"
    );

    // a customer message: its fields, no ids, no empty lists, the text left to the card's quote
    let msg = json!({"kind": "customer_message", "channel": "email", "to": "hiroshi@clinic.example.com",
                     "cc": [], "bcc": [], "subject": "Re: Waiting-room furniture", "text": "Dear Hiroshi",
                     "summary": "Send email", "thread_id": Uuid::new_v4().to_string(),
                     "contact_id": contact_id.to_string()});
    let diff = describe(
        &mut db,
        "mcp.proposal",
        json!({"proposal": msg, "client": "Desktop AI client", "proposer_id": sa.id.to_string()}),
    )
    .await;
    no_nested_proposal(&diff);
    assert_eq!(
        diff["title"],
        json!(format!("Email reply to Sora {last}")),
        "{diff}"
    );
    let after = diff["after"].as_object().unwrap();
    assert_eq!(after["subject"], "Re: Waiting-room furniture", "{diff}");
    assert_eq!(after["to"], "hiroshi@clinic.example.com", "{diff}");
    for hidden in [
        "thread_id",
        "contact_id",
        "cc",
        "bcc",
        "text",
        "kind",
        "summary",
    ] {
        assert!(!after.contains_key(hidden), "{hidden}: {diff}");
    }

    // a quotation: its lines
    let quote = json!({"kind": "quotation", "total": 200000.0, "currency": "JPY", "customer": org,
                       "payload": {"subject": "Printers", "currency": "JPY",
                                   "items": [{"name": "Colour MFP", "quantity": 2.0, "unit_price": 100000.0}]}});
    let diff = describe(
        &mut db,
        "mcp.proposal",
        json!({"proposal": quote, "proposer_id": sa.id.to_string()}),
    )
    .await;
    no_nested_proposal(&diff);
    assert_eq!(diff["after"]["lines"], json!(["2 × Colour MFP"]), "{diff}");
    assert_eq!(diff["after"]["customer"], json!(org), "{diff}");
    assert_eq!(
        diff["title"],
        json!(format!("Quotation draft for {org}")),
        "{diff}"
    );

    // a merge: its own title
    let merge = json!({"kind": "nary_action", "action": "organization.merge",
                       "payload": {"keep_id": org_id.to_string(), "merge_ids": [Uuid::new_v4().to_string()]}});
    let diff = describe(
        &mut db,
        "mcp.proposal",
        json!({"proposal": merge, "proposer_id": sa.id.to_string()}),
    )
    .await;
    assert_eq!(diff["title"], "Merge organizations", "{diff}");
    no_nested_proposal(&diff);
}

#[tokio::test]
async fn an_agents_card_carries_the_reason_it_gave() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let actor = granary_services::nary::roles::load_actor(&mut db, sa.id)
        .await
        .unwrap()
        .unwrap();
    let proposal = json!({"kind": "followup_reminder", "title": u("Chase Ginkgo"), "note": "No answer",
                          "due_at": "2026-11-24T10:00:00+09:00", "entity_type": null, "entity_id": null});
    let card = granary_services::ai::mcp_access::as_card(
        &mut db,
        &actor,
        &proposal,
        Some("Desktop AI client"),
        &json!({"reason": "Sent 3 days ago with no answer"}),
    )
    .await
    .unwrap();
    let rationale = card.rationale.unwrap();
    assert!(
        rationale.starts_with("Sent 3 days ago with no answer\n") && rationale.contains("(MCP)"),
        "{rationale}"
    );
    assert_eq!(card.diff.unwrap()["title"], "Follow-up reminder");
}

#[tokio::test]
async fn the_invoice_list_names_the_customer_of_an_invoice_made_without_a_bill_to_name() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (org_id, org, contact_id, last) = customer(&mut db).await;
    // "New invoice" sends the organization and contact, no customer_name
    let r = app
        .post(
            &sa.token,
            "/api/v1/finance/invoices",
            json!({"invoice_type": "proforma", "currency": "JPY", "customer_org_id": org_id.to_string(),
                   "customer_contact_id": contact_id.to_string(),
                   "items": [{"name": "Colour MFP", "quantity": 1, "unit_price": 100000}]}),
        )
        .await;
    assert!(r.status == 200 || r.status == 201, "{}", r.text());
    let made = r.json()["id"].as_str().unwrap().to_string();
    // the snapshot is kept from now on
    let stored: Option<String> =
        sqlx::query_scalar("SELECT customer_name FROM invoices WHERE id = $1::uuid")
            .bind(&made)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some(org.as_str()));
    // an invoice made before (no snapshot), to a contact only: the list names the contact
    sqlx::query(
        "UPDATE invoices SET customer_name = NULL, customer_org_id = NULL WHERE id = $1::uuid",
    )
    .bind(&made)
    .execute(&mut db)
    .await
    .unwrap();
    let rows = app
        .get(&sa.token, "/api/v1/finance/invoices?limit=200")
        .await
        .json();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(made))
        .unwrap_or_else(|| panic!("{rows}"))
        .clone();
    assert_eq!(row["customer_name"], json!(format!("Sora {last}")), "{row}");
    assert_eq!(
        row["customer_contact"],
        json!({"id": contact_id.to_string(), "full_name": format!("Sora {last}")}),
        "{row}"
    );
    assert!(row["customer_org"].is_null(), "{row}");
}

#[tokio::test]
async fn the_kickoff_card_names_the_project_pm_customer_and_tasks_without_nulls() {
    let app = app(TAG);
    let mut db = app.db().await;
    let (org_id, org, contact_id, _) = customer(&mut db).await;
    let deal_id = Uuid::new_v4();
    let title = u("Hoshino meeting rooms");
    sqlx::query(
        "INSERT INTO deals (id, title, contact_id, organization_id, stage, currency, value, probability, \
         product_interests, product_line_ids, custom, is_deleted) \
         VALUES ($1, $2, $3, $4, 'won', 'JPY', 100000, 100, '{}', '{}', '{}', false)",
    )
    .bind(deal_id)
    .bind(&title)
    .bind(contact_id)
    .bind(org_id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO nary_insights (id, kind, deal_id, content, status) VALUES ($1, 'handoff', $2, $3, 'draft')",
    )
    .bind(Uuid::new_v4())
    .bind(deal_id)
    .bind(json!({"summary": "Two rooms, AV and furniture.", "scope_sold": ["AV"], "scope_not_sold": null,
                 "success_criteria": [{"quote": "Ready by March", "source": "call 3 Nov"}], "commitments": null,
                 "sensitivities": [], "red_flags": null, "contacts": null, "open_items": null}))
    .execute(&mut db)
    .await
    .unwrap();
    let diff = describe(
        &mut db,
        "handoff.kickoff",
        json!({"deal_id": deal_id.to_string()}),
    )
    .await;
    assert_eq!(
        diff["label"],
        json!(format!("Start the project for {title}"))
    );
    let after = diff["after"].as_object().unwrap();
    assert!(after.get("package").is_none(), "{diff}");
    assert_eq!(after["project"], json!(title));
    assert_eq!(after["customer"], json!(org));
    assert!(
        after["pm"].as_str().is_some_and(|p| !p.is_empty()),
        "{diff}"
    );
    assert_eq!(after["handoff_summary"], "Two rooms, AV and furniture.");
    assert_eq!(
        after["handoff"],
        json!({"scope_sold": ["AV"], "success_criteria": ["Ready by March (call 3 Nov)"]}),
        "only the filled sections: {diff}"
    );
    let tasks: Option<String> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'nary.handoff.kickoff_tasks'",
    )
    .fetch_optional(&mut db)
    .await
    .unwrap();
    let tasks: Vec<String> = tasks
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if tasks.is_empty() {
        assert!(after.get("kickoff_tasks").is_none(), "{diff}");
    } else {
        assert_eq!(after["kickoff_tasks"], json!(tasks), "{diff}");
    }
    assert!(!diff.to_string().contains("null"), "no nulls: {diff}");
}
