//! Quotation notes templates: the house notes of a quotation from a template per kind of sale (products, services,
//! subscription), and the notes check.
//!
//! - detection (subscription, services, products; first match wins, products the fallback) and the foreign
//!   currency variant (Incoterm and Freight, the export Tax wording)
//! - the filled HTML byte for byte (products), the deal clauses merged in the house order, house clauses guarded
//!   (notes_override only for the Owner or sales:apply_discount)
//! - the check: empty, plain text, a missing clause, a discount without the permission; the approval refuses those
//!   drafts and lets a good one through; sent ones are never re-checked
//! - propose_quotation_draft / propose_quotation_update with notes_deal over MCP at full power and in the chat
//!   (the card confirmed) while nary.paused is true; quotation_notes_template; the editor routes; Settings refuses
//!   a broken template

mod support;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

use granary_services::quotation_notes as qn;

const TAG: &str = "quotation_notes_nov04";
const API: &str = "/api/v1";

async fn auth(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .unwrap()
}

fn line(sku: &str, ptype: Option<&str>, category: Option<&str>) -> qn::NoteLine {
    qn::NoteLine {
        sku: Some(sku.into()),
        name: sku.into(),
        product_type: ptype.map(str::to_string),
        category: category.map(str::to_string),
        typed: false,
        quantity: Decimal::ONE,
        amount: Decimal::from(10_000_000),
        ..Default::default()
    }
}

fn typed(name: &str) -> qn::NoteLine {
    qn::NoteLine {
        name: name.into(),
        typed: true,
        quantity: Decimal::ONE,
        amount: Decimal::from(5_000_000),
        ..Default::default()
    }
}

fn facts(lines: Vec<qn::NoteLine>, currency: &str, subject: &str) -> qn::Facts {
    qn::Facts {
        lines,
        currency: currency.into(),
        // the test templates' home currency
        home_currency: "JPY".into(),
        subject: subject.into(),
        validity_days: 30,
        family: None,
    }
}

fn deal(v: Value) -> qn::NotesArgs {
    qn::NotesArgs {
        deal: v.as_object().cloned().unwrap_or_default(),
        overrides: Default::default(),
        language: "en".into(),
        family: None,
        ..Default::default()
    }
}

/// A catalog product priced in JPY and USD.
async fn product(
    db: &mut PgConnection,
    sku: &str,
    ptype: &str,
    category: &str,
    sale_model: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, category, unit, sale_model, price_metric, \
         pricing_mode, unit_equivalents, is_active, is_orderable, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 'Unit', $6, 'unit', 'list', 1, true, true, false, false)",
    )
    .bind(id)
    .bind(u(sku))
    .bind(format!("{sku} product"))
    .bind(ptype)
    .bind(category)
    .bind(sale_model)
    .execute(&mut *db)
    .await
    .unwrap();
    for (cur, amount) in [("JPY", "10000000"), ("USD", "650")] {
        sqlx::query(
            "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, $3, $4, '2020-01-01')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(cur)
        .bind(amount.parse::<Decimal>().unwrap())
        .execute(&mut *db)
        .await
        .unwrap();
    }
    id
}

/// A customer quoted without tax (so a default sales tax never disagrees with the notes).
async fn customer(db: &mut PgConnection) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let name = u("Notes Customer Ltd");
    sqlx::query(
        "INSERT INTO organizations (id, name, country, type, revenue_currency, tags, is_deleted, custom, \
         quote_without_tax) VALUES ($1, $2, 'United Kingdom', '{}', 'JPY', '{}', false, '{}', true)",
    )
    .bind(id)
    .bind(&name)
    .execute(&mut *db)
    .await
    .unwrap();
    (id, name)
}

/// A draft from the editor: (id, full number).
async fn draft(app: &TestApp, token: &str, org: Uuid, pid: Uuid, notes: &str) -> (String, String) {
    let r = app
        .post(
            token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "JPY", "subject": u("Laptops for the office"), "validity_days": 30,
                   "customer_org_id": org.to_string(), "notes": notes,
                   "items": [{"product_id": pid.to_string(), "quantity": 2}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    (
        r.json()["id"].as_str().unwrap().to_string(),
        r.json()["full_number"].as_str().unwrap().to_string(),
    )
}

async fn notes_of(db: &mut PgConnection, qid: &str) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT notes FROM quotations WHERE id = $1::uuid")
        .bind(qid)
        .fetch_one(&mut *db)
        .await
        .unwrap()
        .unwrap_or_default()
}

async fn set_notes(db: &mut PgConnection, qid: &str, notes: &str) {
    sqlx::query("UPDATE quotations SET notes = $2 WHERE id = $1::uuid")
        .bind(qid)
        .bind(notes)
        .execute(&mut *db)
        .await
        .unwrap();
}

async fn tool(app: &'static TestApp, user_id: Uuid, name: &'static str, args: Value) -> Value {
    app.on_server(async move {
        let mut db = app.db().await;
        let user = auth(&mut db, user_id).await;
        granary_services::ai::tools::catalog::run_tool(&mut db, &user, name, args).await
    })
    .await
}

async fn mcp_token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("nov04"), "scopes": scopes, "password": "pytest-pass"}),
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

/// The superadmin as the Owner (Nary's approver), Nary paused.
async fn world(app: &TestApp) -> support::TestUser {
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    app.set_setting("nary.paused", "true").await;
    sa
}

// ─── detection, the filled HTML, the deal clauses ─────────────────────────────

#[tokio::test]
async fn detects_each_family_first_match_wins_and_the_export_variant() {
    let app = app(TAG);
    let mut db = app.db().await;
    let set = qn::load(&mut db).await.unwrap();
    let fam = |f: &qn::Facts| qn::detect(&set, f).family.clone();
    let mut plan = line("PLAN-TEAM", Some("software"), None);
    plan.sale_model = Some("recurring".into());
    let cases = [
        (
            facts(vec![line("LAPTOP-14", Some("physical"), None)], "JPY", ""),
            "products",
        ),
        (
            facts(vec![line("APP-LICENCE", Some("software"), None)], "USD", ""),
            "products",
        ),
        (
            facts(
                vec![
                    line("LAPTOP-14", Some("physical"), None),
                    line("SETUP-DAY", Some("service"), None),
                ],
                "JPY",
                "",
            ),
            "services",
        ),
        (
            facts(
                vec![
                    line("LAPTOP-14", Some("physical"), None),
                    typed("Implementation and training"),
                ],
                "JPY",
                "",
            ),
            "services",
        ),
        // a recurring line makes it a subscription, whatever else is on the quotation (first match wins)
        (
            facts(
                vec![
                    line("LAPTOP-14", Some("physical"), None),
                    line("SETUP-DAY", Some("service"), None),
                    plan.clone(),
                ],
                "JPY",
                "",
            ),
            "subscription",
        ),
        (facts(vec![typed("Consulting day")], "JPY", ""), "services"),
        // nothing matches: the fallback
        (facts(vec![typed("Office chair")], "JPY", ""), "products"),
    ];
    for (f, want) in &cases {
        assert_eq!(
            fam(f),
            *want,
            "{:?}",
            f.lines.iter().map(|l| &l.name).collect::<Vec<_>>()
        );
    }

    // a foreign currency: the same house order, Incoterm after Currency, the export Currency and Tax wording
    let usd = facts(vec![line("LAPTOP-14", Some("physical"), None)], "USD", "");
    let b = qn::build(
        &mut db,
        &set,
        &usd,
        &deal(json!({"scope": "two laptops for the sales team"})),
    )
    .await
    .unwrap()
    .unwrap();
    let p = qn::parse(&b.html).unwrap();
    let labels: Vec<&str> = p.items.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        &labels[..4],
        &["Validity", "Currency", "Incoterm and Freight", "Scope"],
        "{labels:?}"
    );
    assert!(b
        .html
        .contains("All prices are stated in United States Dollars (USD) and are net prices."));
    assert!(b.html.contains("Prices are quoted ex works."));
    assert!(b
        .html
        .contains("Prices exclude sales tax, import duty, customs clearance"));
    assert_eq!(labels.last(), Some(&"How to Order"));
    assert!(b.html.contains(
        "Delivery within 10 to 15 working days after the order and its payment are confirmed."
    ));
    assert!(!b.html.contains('\u{2014}'));
    // the home currency has no Incoterm; software in a foreign currency ships nothing
    let jpy = facts(vec![line("LAPTOP-14", Some("physical"), None)], "JPY", "");
    let b = qn::build(&mut db, &set, &jpy, &deal(json!({})))
        .await
        .unwrap()
        .unwrap();
    assert!(!b.html.contains("Incoterm"));
    assert_eq!(b.missing, vec!["Scope"]);
    let sw = facts(vec![line("APP-LICENCE", Some("software"), None)], "USD", "");
    let b = qn::build(&mut db, &set, &sw, &deal(json!({})))
        .await
        .unwrap()
        .unwrap();
    assert!(b.html.contains("<li><b>Incoterm and Freight: </b>Software and services are delivered remotely; no shipment.</li>"));
    assert!(
        !b.html.contains("Warranty"),
        "no goods, no warranty: {}",
        b.html
    );
}

#[tokio::test]
async fn the_products_notes_are_the_house_html_byte_for_byte() {
    let app = app(TAG);
    let mut db = app.db().await;
    let set = qn::load(&mut db).await.unwrap();
    let f = facts(vec![line("LAPTOP-14", Some("physical"), None)], "JPY", "");
    let b = qn::build(
        &mut db,
        &set,
        &f,
        &deal(json!({"scope": "two 14-inch laptops for the sales team, delivered to the head office"})),
    )
    .await
    .unwrap()
    .unwrap();
    let want = concat!(
        "<div><ol>",
        "<li><b>Validity: </b>This quotation is valid for 30 days from the issue date.</li>",
        "<li><b>Currency: </b>All prices are stated in Japanese Yen (JPY) and are net prices.</li>",
        "<li><b>Scope: </b>Two 14-inch laptops for the sales team, delivered to the head office.</li>",
        "<li><b>Delivery: </b>Delivery within 10 to 15 working days after the order and its payment are confirmed.</li>",
        "<li><b>Payment Terms: </b>100% upfront on purchase order. Each invoice is payable within 14 days of the invoice date.</li>",
        "<li><b>Tax: </b>Prices exclude sales tax and withholding tax.</li>",
        "<li><b>Warranty: </b>Goods carry a 1 year warranty against manufacturing defects from delivery.</li>",
        "<li><b>How to Order: </b>To order, please send a signed copy of this quotation or a purchase order by email.</li>",
        "</ol></div>",
    );
    assert_eq!(b.html, want);
    assert_eq!(b.family, "products");
    assert_eq!(b.written, vec!["Scope"]);
    assert!(b.missing.is_empty() && b.warnings.is_empty(), "{b:?}");
    assert_eq!(
        b.payment_summary,
        "100% upfront on purchase order. Each invoice is payable within 14 days of the invoice date."
    );
    // the house notes pass their own check
    let c = qn::check_notes(&set, &f, Some(&b.html), false, None);
    assert!(c.issues.is_empty(), "{:?}", c.issues);

    // a subscription: billed yearly in advance, the non-payment clause
    let mut plan = line("PLAN-TEAM", Some("software"), None);
    plan.sale_model = Some("recurring".into());
    let f = facts(vec![plan], "JPY", "");
    let b = qn::build(
        &mut db,
        &set,
        &f,
        &deal(json!({"scope": "a team plan for 25 users"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(b.family, "subscription");
    assert!(b.html.contains("<li><b>Payment Terms: </b>Subscriptions and annual licences are billed yearly in advance. Each invoice is payable within 14 days of the invoice date.</li>"), "{}", b.html);
    assert!(b.html.contains("<li><b>Non-payment: </b>Service is paused when an invoice is 30 days overdue and may be terminated after 60 days.</li>"), "{}", b.html);
    let c = qn::check_notes(&set, &f, Some(&b.html), false, None);
    assert!(c.issues.is_empty(), "{:?}", c.issues);
}

#[tokio::test]
async fn deal_clauses_merge_in_house_order_and_house_clauses_are_guarded() {
    let app = app(TAG);
    let mut db = app.db().await;
    let set = qn::load(&mut db).await.unwrap();
    // services: goods + a service line, so the client obligations, the acceptance and the milestones
    let f = facts(
        vec![
            line("LAPTOP-14", Some("physical"), None),
            line("SETUP-DAY", Some("service"), None),
        ],
        "JPY",
        "",
    );
    let b = qn::build(
        &mut db,
        &set,
        &f,
        &deal(json!({
            "scope": "ten laptops with setup & training",
            "acceptance": "the work is accepted when every laptop is enrolled and the training is held",
            "client_obligations": "(a) a list of the users; (b) a room for the training",
            "payment_milestones": "Milestone 2 (40%): on delivery. Milestone 3 (10%): on acceptance signed by both parties",
            "payment_terms": "Late payments carry no interest",
        })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(b.family, "services");
    let p = qn::parse(&b.html).unwrap();
    let labels: Vec<&str> = p.items.iter().map(|(l, _)| l.as_str()).collect();
    let pos = |l: &str| {
        labels
            .iter()
            .position(|x| *x == l)
            .unwrap_or_else(|| panic!("{l} in {labels:?}"))
    };
    assert!(pos("Scope") < pos("Client Obligations"));
    assert!(pos("Client Obligations") < pos("Acceptance"));
    assert!(pos("Acceptance") < pos("Payment Terms") && pos("Payment Terms") < pos("Tax"));
    assert_eq!(labels.last(), Some(&"How to Order"));
    assert!(
        !labels.contains(&"Payment Milestones"),
        "the milestones are written into Payment Terms"
    );
    assert!(b.html.contains("setup &amp; training"));
    assert!(b.html.contains(
        "Products and licences: 100% upfront on purchase order. Services: 50% down payment on purchase order or contract signing; work begins only after this payment clears. Milestone 2 (40%): on delivery. Milestone 3 (10%): on acceptance signed by both parties. Each invoice is payable within 14 days of the invoice date. Late payments carry no interest.</li>"
    ), "{}", b.html);
    assert_eq!(b.extended, vec!["Payment Terms"]);
    let c = qn::check_notes(&set, &f, Some(&b.html), false, None);
    assert!(c.issues.is_empty(), "{:?}", c.issues);

    // a house clause can't be rewritten through notes_deal; an unknown key is named
    let e = qn::build(&mut db, &set, &f, &deal(json!({"tax": "No tax at all"})))
        .await
        .unwrap()
        .unwrap_err();
    assert!(e.contains("notes_override"), "{e}");
    let e = qn::build(&mut db, &set, &f, &deal(json!({"warranty_terms": "x"})))
        .await
        .unwrap()
        .unwrap_err();
    assert!(e.contains("not a clause") && e.contains("scope"), "{e}");
    // notes_override replaces it (the permission is the tool's)
    let mut o = deal(json!({"scope": "x"}));
    o.overrides = json!({"payment_terms": "60% on purchase order, 40% on go-live."})
        .as_object()
        .cloned()
        .unwrap();
    let b = qn::build(&mut db, &set, &f, &o).await.unwrap().unwrap();
    assert!(b
        .html
        .contains("<li><b>Payment Terms: </b>60% on purchase order, 40% on go-live.</li>"));
    assert_eq!(b.overridden, vec!["Payment Terms"]);
}

// ─── the check ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_check_finds_missing_plain_and_discount() {
    let app = app(TAG);
    let mut db = app.db().await;
    let set = qn::load(&mut db).await.unwrap();
    let f = facts(vec![line("LAPTOP-14", Some("physical"), None)], "JPY", "");
    let good = qn::build(&mut db, &set, &f, &deal(json!({"scope": "two laptops"})))
        .await
        .unwrap()
        .unwrap()
        .html;
    let codes = |notes: Option<&str>, discount: bool| -> Vec<&'static str> {
        qn::check_notes(&set, &f, notes, discount, None)
            .issues
            .into_iter()
            .map(|i| i.code)
            .collect()
    };
    assert!(codes(Some(&good), false).is_empty());
    assert_eq!(codes(None, false), vec!["empty"]);
    assert_eq!(
        codes(Some("Prices exclude sales tax and withholding tax."), false),
        vec!["not_numbered"]
    );
    let no_scope = good.replace("<li><b>Scope: </b>Two laptops.</li>", "");
    assert_ne!(no_scope, good);
    let c = qn::check_notes(&set, &f, Some(&no_scope), false, None);
    assert_eq!(c.missing, vec!["Scope"]);
    let add = |item: &str| good.replace("</ol></div>", &format!("{item}</ol></div>"));
    let disc =
        add("<li><b>Price Basis: </b>A 10% discount applies for orders confirmed this month.</li>");
    assert_eq!(codes(Some(&disc), false), vec!["discount"]);
    assert!(codes(Some(&disc), true).is_empty());
    // a <strong> label from the editor is fine
    assert!(codes(
        Some(&good.replace("<b>", "<strong>").replace("</b>", "</strong>")),
        false
    )
    .is_empty());
}

#[tokio::test]
async fn the_approval_refuses_failing_notes_and_takes_good_ones() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = world(app).await;
    app.set_setting(qn::BLOCK_KEY, "true").await;
    let mut db = app.db().await;
    let pid = product(&mut db, "LAPTOP-T", "physical", "hardware", "one_time").await;
    let (org, _) = customer(&mut db).await;
    let sales = app
        .user_with(&[
            "sales:create_quotation",
            "sales:view_quotation",
            "sales:edit_quotation",
        ])
        .await;
    let seller = auth(&mut db, sales.id).await;
    let owner = auth(&mut db, sa.id).await;
    let (qid, number) = draft(
        app,
        &sa.token,
        org,
        pid,
        "Prices exclude sales tax and withholding tax.",
    )
    .await;
    use granary_services::approvals::engine;
    let refusal = |r: Result<
        granary_db::rows::approval::ApprovalRequest,
        granary_core::ApiError,
    >|
     -> String {
        match r {
            Err(granary_core::ApiError::Http { detail, .. }) => {
                detail.as_str().unwrap_or("").to_string()
            }
            other => panic!("not refused: {:?}", other.map(|r| r.id)),
        }
    };
    // plain text
    let why = refusal(engine::submit(&mut db, "quotation", &qid, &seller, None, None, None).await);
    assert!(
        why.contains("house template") && why.contains("not a numbered list"),
        "{why}"
    );
    // the house notes without the deal clauses: the missing ones are named
    let s = qn::stored(&mut db, Uuid::parse_str(&qid).unwrap())
        .await
        .unwrap()
        .unwrap();
    let set = qn::load(&mut db).await.unwrap();
    let bare = qn::build(&mut db, &set, &s.facts, &deal(json!({})))
        .await
        .unwrap()
        .unwrap()
        .html;
    set_notes(&mut db, &qid, &bare).await;
    let why = refusal(engine::submit(&mut db, "quotation", &qid, &seller, None, None, None).await);
    assert!(why.contains("missing") && why.contains("Scope"), "{why}");
    let good = qn::build(
        &mut db,
        &set,
        &s.facts,
        &deal(json!({"scope": "two laptops"})),
    )
    .await
    .unwrap()
    .unwrap()
    .html;
    for (bad, word) in [(
        "<li><b>Price Basis: </b>A 5% discount applies.</li>",
        "discount",
    )] {
        set_notes(
            &mut db,
            &qid,
            &good.replace("</ol></div>", &format!("{bad}</ol></div>")),
        )
        .await;
        let why =
            refusal(engine::submit(&mut db, "quotation", &qid, &seller, None, None, None).await);
        assert!(why.contains(word), "{word}: {why}");
    }
    // the Owner may state a discount
    engine::submit(&mut db, "quotation", &qid, &owner, None, None, None)
        .await
        .unwrap();
    // good notes go for the seller too (a fresh draft); a sent quotation is never re-checked
    let (other, _) = draft(app, &sa.token, org, pid, &good).await;
    engine::submit(&mut db, "quotation", &other, &seller, None, None, None)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE quotations SET status = 'sent', notes = 'Old free notes' WHERE id = $1::uuid",
    )
    .bind(&other)
    .execute(&mut db)
    .await
    .unwrap();
    assert_eq!(
        qn::quotation_refusal(&mut db, Uuid::parse_str(&other).unwrap(), &seller)
            .await
            .unwrap(),
        None
    );
    // the route says it too
    let (third, _) = draft(app, &sa.token, org, pid, "<p>Thanks</p>").await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/approvals/submit"),
            json!({"entity_type": "quotation", "entity_id": third}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("house template"), "{}", r.text());
    // off: the old behaviour
    app.set_setting(qn::BLOCK_KEY, "false").await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/approvals/submit"),
            json!({"entity_type": "quotation", "entity_id": third}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let _ = number;
}

// ─── the tools: chat and MCP full power, Nary paused ──────────────────────────

#[tokio::test]
async fn the_tools_build_the_notes_in_the_chat_and_over_mcp_while_nary_is_paused() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = world(app).await;
    let mut db = app.db().await;
    let pid = product(&mut db, "LAPTOP-C", "physical", "hardware", "one_time").await;
    let (_org, name) = customer(&mut db).await;
    let items = format!("[{{\"product_id\": \"{pid}\", \"quantity\": 2}}]");
    let deal_json = json!({"scope": "two laptops for the warehouse office", "price_basis": "prices follow the current price list"});

    // quotation_notes_template on lines not saved yet
    let t = tool(
        app,
        sa.id,
        "quotation_notes_template",
        json!({"items_json": items, "currency": "JPY", "customer": name}),
    )
    .await;
    assert_eq!(t["family"], "products", "{t}");
    let deal_keys: Vec<&str> = t["deal_clauses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap())
        .collect();
    assert!(
        deal_keys.contains(&"scope") && deal_keys.contains(&"price_basis"),
        "{deal_keys:?}"
    );
    assert!(t["house_clauses"]
        .to_string()
        .contains("To order, please send a signed copy of this quotation"));
    assert_eq!(t["optional_house_clauses"], json!([]), "{t}");

    // the chat: propose_quotation_draft with notes_deal; the card says what was filled; the editor saves it
    let out = tool(
        app,
        sa.id,
        "propose_quotation_draft",
        json!({"customer": name, "currency": "JPY", "items_json": items, "notes_deal": deal_json.to_string()}),
    )
    .await;
    let card = &out["proposal"];
    let notes = card["payload"]["notes"]
        .as_str()
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        notes.starts_with("<div><ol><li><b>Validity: </b>This quotation is valid for 30 days"),
        "{notes}"
    );
    assert!(notes.contains("<li><b>Scope: </b>Two laptops for the warehouse office.</li>"));
    assert_eq!(card["payload"]["validity_days"], 30);
    assert_eq!(card["payload"]["payment_terms"]["text"], "100% upfront on purchase order. Each invoice is payable within 14 days of the invoice date.");
    let summary = card["notes_summary"].as_str().unwrap();
    assert!(
        summary.contains("house clauses filled by the ERP")
            && summary.contains("Scope, Price Basis"),
        "{summary}"
    );
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            card["payload"].clone(),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let qid = r.json()["id"].as_str().unwrap().to_string();
    let number = r.json()["full_number"].as_str().unwrap().to_string();
    assert_eq!(notes_of(&mut db, &qid).await, notes);
    // missing deal clauses are flagged on the card; free notes are checked
    let out = tool(
        app,
        sa.id,
        "propose_quotation_draft",
        json!({"customer": name, "currency": "JPY", "items_json": items}),
    )
    .await;
    assert!(
        out["proposal"]["warnings"]
            .to_string()
            .contains("still miss Scope"),
        "{out}"
    );
    let out = tool(
        app,
        sa.id,
        "propose_quotation_draft",
        json!({"customer": name, "currency": "JPY", "items_json": items, "notes": "Thanks."}),
    )
    .await;
    assert!(
        out["proposal"]["warnings"]
            .to_string()
            .contains("not a numbered list"),
        "{out}"
    );
    let out = tool(
        app,
        sa.id,
        "propose_quotation_draft",
        json!({"items_json": items, "notes": "x", "notes_deal": {"scope": "y"}}),
    )
    .await;
    assert!(out["error"].as_str().unwrap().contains("not both"), "{out}");

    // notes_override: the Owner or sales:apply_discount only
    let seller = app
        .user_with(&["sales:create_quotation", "sales:view_catalog"])
        .await;
    let ov = json!({"customer": name, "currency": "JPY", "items_json": items, "notes_deal": deal_json,
                    "notes_override": {"payment_terms": "60% on purchase order, 40% before shipment."}});
    let out = tool(app, seller.id, "propose_quotation_draft", ov.clone()).await;
    assert_eq!(out["error"], qn::OVERRIDE_REFUSED, "{out}");
    let out = tool(app, sa.id, "propose_quotation_draft", ov).await;
    assert!(
        out["proposal"]["payload"]["notes"]
            .as_str()
            .unwrap()
            .contains("<b>Payment Terms: </b>60% on purchase order, 40% before shipment.</li>"),
        "{out}"
    );

    // the chat: propose_quotation_update rebuilds the notes, the Confirm applies them
    let out = tool(
        app,
        sa.id,
        "propose_quotation_update",
        json!({"quotation_id": number, "notes_deal": {"scope": "three laptops", "price_basis": "prices follow the 2026 price list"}}),
    )
    .await;
    let card = &out["proposal"];
    assert!(
        out["changes"]
            .to_string()
            .contains("Notes from the house template"),
        "{out}"
    );
    let r = app
        .post(
            &sa.token,
            &format!("{API}/nary/actions/run"),
            json!({"action": card["action"], "payload": card["payload"]}),
        )
        .await;
    assert!(r.status < 300, "{}", r.text());
    let saved = notes_of(&mut db, &qid).await;
    assert!(
        saved.contains("<li><b>Scope: </b>Three laptops.</li>")
            && saved.contains("<li><b>Price Basis: </b>Prices follow the 2026 price list.</li>"),
        "{saved}"
    );
    let terms: Option<Value> =
        sqlx::query_scalar("SELECT payment_terms FROM quotations WHERE id = $1::uuid")
            .bind(&qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(terms.unwrap()["text"], "100% upfront on purchase order. Each invoice is payable within 14 days of the invoice date.");

    // your AI client (MCP) at full power: the tools are listed and the draft is made with the house notes
    let full = mcp_token(app, &sa.token, &["full"]).await;
    let r = app
        .post(
            &full,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
    assert!(r.text().contains("quotation_notes_template") && r.text().contains("notes_deal"));
    let t = call(
        app,
        &full,
        "quotation_notes_template",
        json!({"quotation_id": number}),
    )
    .await;
    assert_eq!(t["family"], "products", "{t}");
    assert_eq!(t["current_notes_check"]["ok"], true, "{t}");
    let subject = u("Notes over MCP");
    let out = call(
        app,
        &full,
        "propose_quotation_draft",
        json!({"customer": name, "currency": "JPY", "subject": subject, "items_json": items, "notes_deal": deal_json}),
    )
    .await;
    assert!(out.get("error").is_none_or(Value::is_null), "{out}");
    let out = if out["approval_required"] == true {
        call(
            app,
            &full,
            "nary_apply",
            json!({"apply_id": out["apply_id"]}),
        )
        .await
    } else {
        out
    };
    assert!(out.get("error").is_none_or(Value::is_null), "{out}");
    let made: Option<String> =
        sqlx::query_scalar("SELECT notes FROM quotations WHERE subject = $1")
            .bind(&subject)
            .fetch_optional(&mut db)
            .await
            .unwrap()
            .flatten();
    let made = made.unwrap_or_else(|| panic!("no quotation made: {out}"));
    assert!(
        made.contains("<li><b>Scope: </b>Two laptops for the warehouse office.</li>"),
        "{made}"
    );
}

// ─── the editor routes and Settings ───────────────────────────────────────────

#[tokio::test]
async fn the_editor_fills_and_checks_and_settings_refuse_a_broken_template() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db, "LAPTOP-E", "physical", "hardware", "one_time").await;
    let (org, _) = customer(&mut db).await;
    let form = json!({"currency": "JPY", "customer_org_id": org.to_string(), "validity_days": 30,
                      "items": [{"product_id": pid.to_string(), "quantity": 1, "tax_pct": 0}]});
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotation-notes/template"),
            form.clone(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let t = r.json();
    assert_eq!(t["family"], "products", "{t}");
    let html = t["preview_html"].as_str().unwrap();
    assert!(
        html.contains("<li><b>Warranty: </b>") && !html.contains("Scope"),
        "{html}"
    );
    assert!(t["payment_terms"]
        .as_str()
        .unwrap()
        .starts_with("100% upfront on purchase order."));
    let mut chk = form.clone();
    chk["notes"] = json!(html);
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotation-notes/check"),
            chk.clone(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["missing"], json!(["Scope"]), "{}", r.text());
    chk["notes"] = json!("Prices exclude sales tax.");
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotation-notes/check"),
            chk,
        )
        .await;
    assert!(r.text().contains("not_numbered"), "{}", r.text());
    let nobody = app.user_with(&["crm:view_own"]).await;
    let r = app
        .post(
            &nobody.token,
            &format!("{API}/sales/quotation-notes/check"),
            form,
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());

    // Settings: a template is checked when saved
    for (value, why) in [
        ("{not json", "Not valid JSON"),
        (
            r#"{"name": "X", "clauses": [{"key": "a", "label": "A", "kind": "fixed", "required": true, "wording_en": "Valid for {nope} days."}]}"#,
            "{nope}",
        ),
        (
            r#"{"name": "X", "clauses": [{"key": "a", "label": "A", "kind": "maybe"}]}"#,
            "fixed or deal",
        ),
        (
            r#"{"name": "X", "clauses": [{"key": "a", "label": "A", "kind": "deal", "when": "tuesdays"}]}"#,
            "is not known",
        ),
    ] {
        let r = app
            .patch(
                &sa.token,
                &format!("{API}/system/settings/sales.quotation.notes_template.products"),
                json!({"value": value}),
            )
            .await;
        assert_eq!(r.status, 400, "{value}: {}", r.text());
        assert!(r.text().contains(why), "{why}: {}", r.text());
    }
    let cur: String = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'sales.quotation.notes_template.products'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/system/settings/sales.quotation.notes_template.products"),
            json!({"value": cur}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

/// The house formats are followed strictly: a revision of a sent quotation whose notes don't
/// follow the house template makes no card; with notes_deal the notes are built from the template.
#[tokio::test]
async fn a_revision_with_off_template_notes_makes_no_card_until_notes_deal() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = world(app).await;
    app.set_setting(qn::BLOCK_KEY, "true").await;
    let mut db = app.db().await;
    let pid = product(&mut db, "LAPTOP-R", "physical", "hardware", "one_time").await;
    let (org, _) = customer(&mut db).await;
    let (qid, _) = draft(
        app,
        &sa.token,
        org,
        pid,
        "Old free notes, before the house template.",
    )
    .await;
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    // kept notes: refused before any card, with what to do
    let out = tool(
        app,
        sa.id,
        "propose_quotation_revision",
        json!({"quotation_id": qid, "validity_days": 30}),
    )
    .await;
    let e = out["error"].as_str().unwrap_or_default();
    assert!(
        e.contains("No card made") && e.contains("notes_deal") && e.contains("kept"),
        "{out}"
    );
    assert!(out.get("proposal").is_none(), "{out}");
    // free notes that miss clauses: refused too
    let out = tool(
        app,
        sa.id,
        "propose_quotation_revision",
        json!({"quotation_id": qid, "validity_days": 30, "notes": "<ol><li>Hi</li></ol>"}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap_or_default()
            .contains("No card made"),
        "{out}"
    );
    // notes and notes_deal together: refused
    let out = tool(
        app,
        sa.id,
        "propose_quotation_revision",
        json!({"quotation_id": qid, "notes": "x", "notes_deal": {"scope": "y"}}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not both"),
        "{out}"
    );
    // notes_deal: the card is made with the house notes
    let out = tool(
        app,
        sa.id,
        "propose_quotation_revision",
        json!({"quotation_id": qid, "validity_days": 30,
               "notes_deal": {"scope": "two laptops"}}),
    )
    .await;
    assert!(out.get("error").is_none_or(Value::is_null), "{out}");
    let card = out.to_string();
    assert!(card.contains("Scope: </b>Two laptops"), "{out}");
    assert!(
        card.contains("Notes: replaced by the house template notes"),
        "{out}"
    );
    app.set_setting(qn::BLOCK_KEY, "false").await;
}

/// Typed delivery / installation lines don't turn a products quotation into a services one;
/// notes_family picks the template by hand and stays with the quotation; an approved draft not sent yet can be
/// changed and goes to approval again.
#[tokio::test]
async fn logistics_lines_keep_the_family_notes_family_sticks_and_an_approved_draft_changes() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = world(app).await;
    let mut db = app.db().await;
    let laptop = product(&mut db, "LAPTOP-L", "physical", "hardware", "one_time").await;
    let (org, _) = customer(&mut db).await;
    let make = |typed: &'static str| {
        json!({"currency": "JPY", "subject": u("Office laptops"), "validity_days": 30,
               "customer_org_id": org.to_string(), "notes": "",
               "items": [{"product_id": laptop.to_string(), "quantity": 2},
                         {"name": typed, "quantity": 1, "unit_price": 2000000},
                         {"name": "Delivery to site", "quantity": 1, "unit_price": 1500000}]})
    };
    let family_of = |db: &'static TestApp, qid: String| async move {
        let mut c = db.db().await;
        let s = qn::stored(&mut c, Uuid::parse_str(&qid).unwrap())
            .await
            .unwrap()
            .unwrap();
        let set = qn::load(&mut c).await.unwrap();
        qn::detect(&set, &s.facts).family.clone()
    };
    // delivery + installation next to a laptop: the products family
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            make("Installation and commissioning"),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let qid = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(family_of(app, qid.clone()).await, "products");
    // a typed integration line still makes it a services quotation
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations"),
            make("Custom integration with the CRM"),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let pid = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(family_of(app, pid).await, "services");

    // notes_family by hand on an update card, saved with the quotation when applied
    let out = tool(
        app,
        sa.id,
        "propose_quotation_update",
        json!({"quotation_id": qid, "notes_family": "nonsense"}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not known"),
        "{out}"
    );
    let payload = json!({"quotation_id": qid, "notes_family": "services",
        "notes": "<div><ol><li><b>Scope: </b>x.</li></ol></div>"});
    assert!(
        granary_services::nary::actions::get("quotation.update")
            .unwrap()
            .validate(&payload)
            .is_ok(),
        "the update action takes notes_family"
    );
    sqlx::query("UPDATE quotations SET notes_family = 'services' WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(family_of(app, qid.clone()).await, "services");
    sqlx::query("UPDATE quotations SET notes_family = NULL WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();

    // an approved draft not sent yet: the update card is made and says it goes to approval again
    sqlx::query(
        "INSERT INTO approval_requests (id, entity_type, entity_id, status, document_revision, snapshot, flags, \
         steps, submitted_by_id, submitted_at) \
         VALUES (gen_random_uuid(), 'quotation', $1::uuid, 'approved', 1, '{}', '[]', '[]', $2, now())",
    )
    .bind(&qid)
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    let out = tool(
        app,
        sa.id,
        "propose_quotation_update",
        json!({"quotation_id": qid, "subject": u("Office laptops, 2 units")}),
    )
    .await;
    assert!(out.get("error").is_none_or(Value::is_null), "{out}");
    assert!(out.to_string().contains("goes to approval again"), "{out}");
}

/// An approved draft not sent reads "approved" (badge, API, Nary's
/// tools); its notes are part of the approval, so a notes edit makes it stale (an approval made before this keeps
/// its own facts); payment_terms_text sets the field and the Payment Terms clause together; a notes edit keeps the
/// clauses it does not mention.
#[tokio::test]
async fn approved_reads_approved_notes_bind_it_and_payment_terms_are_one_edit() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = world(app).await;
    let mut db = app.db().await;
    let pid = product(&mut db, "LAPTOP-T", "physical", "hardware", "one_time").await;
    let (org, _) = customer(&mut db).await;
    let owner = auth(&mut db, sa.id).await;
    use granary_services::approvals::engine;
    let qid0 = draft(app, &sa.token, org, pid, "").await.0;
    let s = qn::stored(&mut db, Uuid::parse_str(&qid0).unwrap())
        .await
        .unwrap()
        .unwrap();
    let set = qn::load(&mut db).await.unwrap();
    let good = qn::build(
        &mut db,
        &set,
        &s.facts,
        &deal(json!({"scope": "two laptops", "price_basis": "prices follow the 2026 price list"})),
    )
    .await
    .unwrap()
    .unwrap()
    .html;
    let (qid, _) = draft(app, &sa.token, org, pid, &good).await;
    let id = Uuid::parse_str(&qid).unwrap();
    let shown = |app: &'static TestApp, qid: String, token: String| async move {
        let r = app
            .get(&token, &format!("{API}/sales/quotations/{qid}"))
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
        r.json()["display_status"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(shown(app, qid.clone(), sa.token.clone()).await, "draft");

    // approved: the API, the list and Nary's tools say "approved"; the stored status stays draft
    let req = engine::submit(&mut db, "quotation", &qid, &owner, None, None, None)
        .await
        .unwrap();
    if req.status != "approved" {
        engine::act(
            &mut db,
            &req.id.to_string(),
            &owner,
            "approved",
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }
    // the engine leaves its transaction open (the routes commit): commit it so the API sees the approval
    sqlx::query("COMMIT").execute(&mut db).await.unwrap();
    assert_eq!(shown(app, qid.clone(), sa.token.clone()).await, "approved");
    let r = app
        .get(&sa.token, &format!("{API}/sales/quotations?limit=500"))
        .await;
    let row = r
        .json()
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["id"] == qid.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["display_status"], "approved");
    assert_eq!(row["status"], "draft");
    let got = tool(app, sa.id, "get_quotation", json!({"quotation_id": qid})).await;
    assert_eq!(got["status"], "approved", "{got}");
    assert_eq!(got["approval_state"], "approved", "{got}");
    let listed = tool(
        app,
        sa.id,
        "list_quotations",
        json!({"status": "approved", "limit": 50}),
    )
    .await;
    assert!(listed.to_string().contains(&qid), "{listed}");
    assert_eq!(
        engine::quotation_display_status(&mut db, id, "draft")
            .await
            .unwrap(),
        "approved"
    );

    // an approval made before notes were part of it (its snapshot has no notes) is not made stale by them
    let snap: Value = sqlx::query_scalar("SELECT snapshot FROM approval_requests WHERE id = $1")
        .bind(req.id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(
        snap["document"].get("notes").is_some(),
        "new approvals cover the notes"
    );
    let mut old = snap.clone();
    assert_eq!(snap["facts_bound"], true, "new approvals are marked");
    old["document"].as_object_mut().unwrap().remove("notes");
    old.as_object_mut().unwrap().remove("facts_bound");
    old["fingerprint"] = json!(engine::facts_fingerprint(&old["document"], &[]));
    sqlx::query("UPDATE approval_requests SET snapshot = $2 WHERE id = $1")
        .bind(req.id)
        .bind(&old)
        .execute(&mut db)
        .await
        .unwrap();
    let edited = good.replace(
        "</ol></div>",
        "<li><b>Training: </b>One day on site.</li></ol></div>",
    );
    assert_ne!(edited, good);
    set_notes(&mut db, &qid, &edited).await;
    assert_eq!(
        engine::gate_state(&mut db, "quotation", id).await.unwrap(),
        "approved"
    );
    // a new approval covers them: a notes edit makes it stale, and it reads draft again
    sqlx::query("UPDATE approval_requests SET snapshot = $2 WHERE id = $1")
        .bind(req.id)
        .bind(&snap)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        engine::gate_state(&mut db, "quotation", id).await.unwrap(),
        "stale"
    );
    assert_eq!(shown(app, qid.clone(), sa.token.clone()).await, "draft");
    let got = tool(app, sa.id, "get_quotation", json!({"quotation_id": qid})).await;
    assert_eq!(got["status"], "draft");
    assert_eq!(got["approval_state"], "stale");
    set_notes(&mut db, &qid, &good).await;

    // payment_terms_text: one edit sets the field and the notes' Payment Terms clause; the deal clauses stay
    let out = tool(
        app,
        sa.id,
        "propose_quotation_update",
        json!({"quotation_id": qid, "payment_terms_text": "100% before the work starts.", "payment_terms_days": 7}),
    )
    .await;
    assert!(out.get("error").is_none_or(Value::is_null), "{out}");
    let text_out = out.to_string();
    assert!(
        text_out.contains("Payment terms:") && text_out.contains("100% before the work starts."),
        "{out}"
    );
    assert!(text_out.contains("goes to approval again"), "{out}");
    let payload = out["proposal"]["payload"].clone();
    let notes = payload["notes"].as_str().unwrap_or_default().to_string();
    assert!(
        notes.contains("100% before the work starts."),
        "the clause: {notes}"
    );
    assert!(
        notes.contains("Two laptops") && notes.contains("Prices follow the 2026 price list"),
        "deal clauses kept: {notes}"
    );
    assert_eq!(
        payload["payment_terms_text"], "100% before the work starts.",
        "{payload}"
    );
    assert_eq!(payload["payment_terms_days"], 7);
    assert!(text_out.contains("kept as they were"), "{out}");

    // notes_override of the clause alone keeps the deal clauses too (the reported bug)
    let out = tool(
        app,
        sa.id,
        "propose_quotation_update",
        json!({"quotation_id": qid, "notes_override": {"payment_terms": "50% on PO, 50% on delivery."}}),
    )
    .await;
    let payload = out["proposal"]["payload"].clone();
    let notes = payload["notes"].as_str().unwrap_or_default().to_string();
    assert!(
        notes.contains("Two laptops") && notes.contains("50% on PO, 50% on delivery."),
        "{out}"
    );
    assert_eq!(
        payload["payment_terms_text"], "50% on PO, 50% on delivery.",
        "the field follows the clause"
    );
    // notes_reset rebuilds from the template: the deal clauses are asked for again
    let out = tool(
        app,
        sa.id,
        "propose_quotation_update",
        json!({"quotation_id": qid, "notes_reset": true}),
    )
    .await;
    assert!(out.to_string().contains("still miss"), "{out}");

    // without the pricing permission the payment terms can't change
    let sales = app
        .user_with(&[
            "sales:create_quotation",
            "sales:view_quotation",
            "sales:edit_quotation",
        ])
        .await;
    let out = tool(
        app,
        sales.id,
        "propose_quotation_update",
        json!({"quotation_id": qid, "payment_terms_text": "Net 90"}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap_or_default()
            .contains("pricing permission"),
        "{out}"
    );
}
