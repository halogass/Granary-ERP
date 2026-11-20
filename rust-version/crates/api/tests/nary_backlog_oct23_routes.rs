//! 23 Oct 2026, Nary's R&D backlog through the real app: the REST routes and your AI client (MCP) at full
//! power, the Owner's connection (scopes ["full"] and ["act", "owner"], the Owner as Nary's approver). The internal
//! paths are crates/services/tests/nary_backlog_oct23.rs.
//!
//! - GET /crm/activities/{id}: a 20,000-character Markdown note saved through POST /crm/activities reads back
//!   identical; another salesperson's record is refused (403); over crm.activity.max_chars POST and PATCH are
//!   refused with a 422, never cut.
//! - MCP at full power: get_activity and render_document_page are listed (and at read level); a long note
//!   made with create_crm_record reads back identical with get_activity in parts; get_activity_history marks
//!   it truncated; an over-long note is refused; render_document_page answers with an MCP image item.
//! - MCP at full power: a WhatsApp message with a proposal and its linked quotation (also given on its own)
//!   is confirmed with the code and sends each PDF once.
//!
//! One wiremock server stands in for the PDF service (a real PDF, so pages can be drawn), MinIO and the
//! WhatsApp service, pointed at by the env before the app starts.

mod support;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use support::{u, TestApp};
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

const TAG: &str = "nary_backlog_oct23";
const API: &str = "/api/v1";

// ─── The fake PDF service, MinIO and WhatsApp ───────────────────────────────────

static STORE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
static WA_TEXT: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static WA_MEDIA: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// A real PDF of `pages` blank A4 pages.
fn real_pdf(pages: usize) -> Vec<u8> {
    let mut d = granary_services::pdftools::Document::new().unwrap();
    for _ in 0..pages {
        d.new_page(595.0, 842.0).unwrap();
    }
    d.tobytes(Default::default()).unwrap()
}

fn key() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_uppercase()
}

struct Fake;

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path().to_string();
        let method = req.method.as_str().to_string();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        if path == "/render" {
            return ResponseTemplate::new(200).set_body_bytes(real_pdf(2));
        }
        if method == "POST" && (path == "/send" || path == "/send-media") {
            let to = body["to"].as_str().unwrap_or("").to_string();
            if path == "/send" {
                WA_TEXT.lock().unwrap().push(body);
            } else {
                WA_MEDIA.lock().unwrap().push(body);
            }
            return ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
                "messageId": format!("true_{to}_{}", key())}));
        }
        if path.starts_with("/contact/") {
            return ResponseTemplate::new(200).set_body_json(
                json!({"isMyContact": false, "isBusiness": false, "pushname": "Bob"}),
            );
        }
        let mut store = STORE.lock().unwrap();
        let store = store.get_or_insert_with(HashMap::new);
        let rest = path.trim_start_matches('/');
        match (method.as_str(), rest.split_once('/')) {
            ("HEAD", None) | ("PUT", None) => ResponseTemplate::new(200),
            ("PUT", Some((_, key))) => {
                store.insert(key.to_string(), req.body.clone());
                ResponseTemplate::new(200)
            }
            ("GET", Some((_, key))) => match store.get(key) {
                Some(b) => ResponseTemplate::new(200).set_body_bytes(b.clone()),
                None => ResponseTemplate::new(404),
            },
            _ => ResponseTemplate::new(404),
        }
    }
}

fn app() -> &'static TestApp {
    static URI: OnceLock<String> = OnceLock::new();
    URI.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let server = wiremock::MockServer::start().await;
                wiremock::Mock::given(wiremock::matchers::any())
                    .respond_with(Fake)
                    .mount(&server)
                    .await;
                tx.send(server.uri()).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let uri = rx.recv().unwrap();
        std::env::set_var("PDF_SERVICE_URL", &uri);
        std::env::set_var("WHATSAPP_SERVICE_URL", &uri);
        std::env::set_var("MINIO_ENDPOINT", uri.trim_start_matches("http://"));
        std::env::set_var("MINIO_USE_SSL", "false");
        std::env::set_var("APP_URL", "http://test.local");
        // 3 Nov 2026: the outbound seal (a self-signed certificate in a scratch path)
        std::env::set_var(
            "SIGN_CERT_P12_PATH",
            std::env::temp_dir().join(format!("granary-api-oct23-{}.p12", std::process::id())),
        );
        uri
    });
    support::app(TAG)
}

// ─── Fixtures ───────────────────────────────────────────────────────────────────

/// The seeded superadmin (SUPERADMIN_EMAIL: the approver of the Owner approval steps, who approves their own
/// submissions on the spot) holds the Owner role (Nary's approver); its token.
async fn owner(app: &TestApp) -> support::TestUser {
    let email = std::env::var("SUPERADMIN_EMAIL").unwrap();
    let mut db = app.db().await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&mut db)
        .await
        .unwrap();
    let sa = support::TestUser {
        id,
        email,
        token: app.token_for(id),
        role_id: None,
    };
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    sa
}

struct Cust {
    contact_id: Uuid,
    org_id: Uuid,
    chat_id: String,
}

/// A contact and organization with an active chat the customer started.
async fn customer(app: &TestApp) -> Cust {
    let mut db = app.db().await;
    let p = format!("8190{}", &Uuid::new_v4().as_u128().to_string()[..8]);
    let org_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, type, revenue_currency, tags, is_deleted, custom) \
         VALUES ($1, $2, '{prospect}', 'USD', '{}', false, '{}')",
    )
    .bind(org_id)
    .bind(u("Globex Group"))
    .execute(&mut db)
    .await
    .unwrap();
    let contact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, email, whatsapp, phone, organization_id, is_primary, \
         tags, is_deleted, custom) VALUES ($1, 'Bob', $2, NULL, $3, $3, $4, false, '{}', false, '{}')",
    )
    .bind(contact_id)
    .bind(u("Pytest"))
    .bind(format!("+{p}"))
    .bind(org_id)
    .execute(&mut db)
    .await
    .unwrap();
    let chat_id = format!("{p}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, \
         contact_id, triage, language, profile_fetched_at, attribution_asked) \
         VALUES ($1, $2, $3, 'Bob', false, 'active', true, $4, 'prospect', 'en', now(), false)",
    )
    .bind(Uuid::new_v4())
    .bind(&chat_id)
    .bind(format!("+{p}"))
    .bind(contact_id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts, contact_id, \
         processed_at) VALUES ($1, $2, $3, 'inbound', false, 'Hello, please send the proposal', \
         now() - interval '1 hour', $4, now())",
    )
    .bind(Uuid::new_v4())
    .bind(key())
    .bind(&chat_id)
    .bind(contact_id)
    .execute(&mut db)
    .await
    .unwrap();
    Cust {
        contact_id,
        org_id,
        chat_id,
    }
}

async fn deal_for(app: &TestApp, cust: &Cust, owner: Option<Uuid>) -> Uuid {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, contact_id, organization_id, stage, currency, value, probability, \
         product_interests, product_line_ids, custom, is_deleted, assigned_to_id) \
         VALUES ($1, $2, $3, $4, 'proposal', 'JPY', 100000000, 20, '{}', '{}', '{}', false, $5)",
    )
    .bind(id)
    .bind(u("Pytest deal"))
    .bind(cust.contact_id)
    .bind(cust.org_id)
    .bind(owner)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query("UPDATE wa_chats SET deal_id = $1 WHERE chat_id = $2")
        .bind(id)
        .bind(&cust.chat_id)
        .execute(&mut db)
        .await
        .unwrap();
    id
}

/// A draft quotation for the deal (a catalog-free line; the Owner approves it at the send): (id, number).
async fn quotation(app: &TestApp, deal: Uuid, cust: &Cust) -> (Uuid, String) {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    let number = u("QUO-2026-10");
    let today = chrono::Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, deal_id, customer_contact_id, customer_org_id, \
         issue_date, valid_until, subtotal, total, payment_terms, discount_value, discount_amount, tax_amount, \
         tax_lines, rounded_total, total_quantity, validity_days, procurement_type, customer_po_accepted_verbally, \
         is_deleted) VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, $5, $6, $7, 30000000, 30000000, \
         '{\"text\": \"NET30\"}', 0, 0, 0, '[]', 0, 0, 30, 'standard', false, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(deal)
    .bind(cust.contact_id)
    .bind(cust.org_id)
    .bind(today)
    .bind(today + chrono::Duration::days(30))
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, name, quantity, unit_price, currency, discount_percentage, \
         discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, 'Laptop Server 25', 2, 15000000, 'JPY', 0, 'none', 30000000, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .execute(&mut db)
    .await
    .unwrap();
    (id, number)
}

/// A Solution Proposal on the deal with this quotation linked: (id, number).
async fn proposal(
    app: &TestApp,
    deal: Uuid,
    status: &str,
    quotation: Option<Uuid>,
) -> (Uuid, String) {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    let number = u("PRP-2026-10");
    sqlx::query(
        "INSERT INTO sales_documents (id, doc_type, number, revision, title, language, status, validity_days, \
         cover, deal_id, is_deleted) VALUES ($1, 'solution_proposal', $2, 1, 'Laptop rollout', 'en', $3, 30, '{}', $4, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(status)
    .bind(deal)
    .execute(&mut db)
    .await
    .unwrap();
    if let Some(q) = quotation {
        sqlx::query(
            "INSERT INTO sales_document_quotations (id, document_id, quotation_id, position) VALUES ($1, $2, $3, 0)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(q)
        .execute(&mut db)
        .await
        .unwrap();
    }
    (id, number)
}

/// Long Markdown, exactly `n` characters, ending with the decision the Owner thought was lost.
fn long_note(n: usize) -> String {
    let mut s = String::from("## Decisions\n\n| Item | Owner |\n|---|---|\n| Scope | Owner |\n\n");
    let tail = "\n5. Reply to Franklin.";
    let mut i = 0;
    loop {
        i += 1;
        let line = format!("{i}. Point {i}: café meeting about the Tokyo site.\n");
        if s.chars().count() + line.chars().count() + tail.chars().count() > n {
            break;
        }
        s.push_str(&line);
    }
    while s.chars().count() + tail.chars().count() < n {
        s.push('x');
    }
    s.push_str(tail);
    s
}

// ─── MCP calls ──────────────────────────────────────────────────────────────────

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    // the seeded superadmin has its own password (support: SUPERADMIN_PASSWORD), made users pytest-pass
    let mut r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("oct23"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    if r.status == 403 {
        r = app
            .post(
                bearer,
                &format!("{API}/mcp/tokens"),
                json!({"name": u("oct23"), "scopes": scopes, "password": "test-superadmin-password-1234"}),
            )
            .await;
    }
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, h: &str, method: &str, params: Value) -> Value {
    let r = app
        .post(
            h,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

/// tools/call: (the JSON of the text item, the image items, isError).
async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, Vec<Value>, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await["result"]
        .clone();
    let content = res["content"].as_array().cloned().unwrap_or_default();
    let text = content[0]["text"].as_str().unwrap_or("").to_string();
    assert!(!text.contains("_images"), "pictures never travel as text");
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    let images: Vec<Value> = content
        .iter()
        .skip(1)
        .filter(|c| c["type"] == "image")
        .cloned()
        .collect();
    (body, images, res.get("isError") == Some(&json!(true)))
}

async fn names(app: &TestApp, h: &str) -> Vec<String> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_long_note_reads_back_in_full_over_rest_and_the_limit_is_a_422() {
    let app = app();
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let cust = customer(app).await;
    let owner = app.user_with(&["crm:view_own", "crm:log_activity"]).await;
    let deal = deal_for(app, &cust, Some(owner.id)).await;
    let note = long_note(20_000);
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/activities"),
            json!({"entity_type": "deal", "entity_id": deal.to_string(), "activity_type": "note",
                   "subject": "Hooli Consulting call", "content": note}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let got = app
        .get(&sa.token, &format!("{API}/crm/activities/{id}"))
        .await;
    assert_eq!(got.status, 200, "{}", got.text());
    let a = got.json();
    assert_eq!(
        a["content"].as_str().unwrap(),
        note,
        "identical to what was saved"
    );
    assert_eq!(a["subject"], json!("Hooli Consulting call"));
    assert!(a["created_by_name"].is_string(), "{a}");
    // the same fields as the list
    let list = app
        .get(
            &sa.token,
            &format!("{API}/crm/activities?entity_type=deal&entity_id={deal}"),
        )
        .await
        .json();
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == json!(id))
        .unwrap()
        .clone();
    assert_eq!(row, a);
    // the timeline still sends the 280-character preview (golden unchanged)
    let tl = app
        .get(&sa.token, &format!("{API}/crm/deals/{deal}/timeline"))
        .await;
    if tl.status == 200 {
        let t = tl.json();
        let e = t["items"]
            .as_array()
            .or(t.as_array())
            .unwrap()
            .iter()
            .find(|x| x["id"] == json!(id))
            .cloned()
            .unwrap();
        assert_eq!(e["preview"].as_str().unwrap().chars().count(), 280);
    }

    // the owner sees it, another salesperson does not
    let r = app
        .get(&owner.token, &format!("{API}/crm/activities/{id}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let other = app.user_with(&["crm:view_own", "crm:log_activity"]).await;
    let r = app
        .get(&other.token, &format!("{API}/crm/activities/{id}"))
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    let r = app
        .get(
            &sa.token,
            &format!("{API}/crm/activities/{}", Uuid::new_v4()),
        )
        .await;
    assert_eq!(r.status, 404);

    // over crm.activity.max_chars: refused, never cut
    app.set_setting("crm.activity.max_chars", "1000").await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/activities"),
            json!({"entity_type": "deal", "entity_id": deal.to_string(), "activity_type": "note",
                   "content": long_note(1001)}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    assert!(r.text().contains("crm.activity.max_chars"), "{}", r.text());
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/crm/activities/{id}"),
            json!({"subject": "Renamed"}),
        )
        .await;
    assert_eq!(
        r.status,
        200,
        "a change that keeps the text passes: {}",
        r.text()
    );
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/crm/activities/{id}"),
            json!({"content": long_note(1001)}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    app.set_setting("crm.activity.max_chars", "200000").await;
}

#[tokio::test]
async fn mcp_client_at_full_power_reads_notes_and_sees_document_pages() {
    let app = app();
    let _g = app.serial().await;
    let sa = owner(app).await;
    let full = token(app, &sa.token, &["full"]).await;
    let act_owner = token(app, &sa.token, &["act", "owner"]).await;
    let read = token(app, &sa.token, &["read"]).await;
    for h in [&full, &act_owner, &read] {
        let n = names(app, h).await;
        for t in [
            "get_activity",
            "get_activity_history",
            "render_document_page",
        ] {
            assert!(n.iter().any(|x| x == t), "{t}");
        }
    }
    assert!(names(app, &full)
        .await
        .iter()
        .any(|x| x == "nary_decide_card"));

    let cust = customer(app).await;
    let deal = deal_for(app, &cust, None).await;
    for h in [&full, &act_owner] {
        // a long note made with create_crm_record (applied at once at full power)
        let note = long_note(20_000);
        let (made, _, err) = call(
            app,
            h,
            "create_crm_record",
            json!({"object_type": "activity", "fields": {"entity_type": "deal", "entity_id": deal.to_string(),
                   "deal_id": deal.to_string(), "activity_type": "note", "subject": "Notes", "content": note}}),
        )
        .await;
        assert!(!err && made["applied"] == json!(true), "{made}");
        let id = made["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{made}"))
            .to_string();
        // the history says it is longer
        let (h1, _, _) = call(
            app,
            h,
            "get_activity_history",
            json!({"object_type": "deal", "record_id": deal.to_string(), "limit": 100}),
        )
        .await;
        let item = h1["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == json!(id))
            .cloned()
            .unwrap_or_else(|| panic!("{h1}"));
        assert_eq!(item["truncated"], json!(true));
        assert_eq!(item["read_with"], json!("get_activity"));
        // get_activity pages through it
        let mut text = String::new();
        let mut offset = 0u64;
        loop {
            let (out, _, err) = call(
                app,
                h,
                "get_activity",
                json!({"activity_id": id, "offset": offset}),
            )
            .await;
            assert!(!err && out.get("error").is_none(), "{out}");
            text.push_str(out["content"].as_str().unwrap());
            match out.get("next_offset").and_then(Value::as_u64) {
                Some(n) => offset = n,
                None => break,
            }
        }
        assert_eq!(text, note);
    }
    // over the limit: the reason comes back
    app.set_setting("crm.activity.max_chars", "1000").await;
    let (out, _, err) = call(
        app,
        &full,
        "create_crm_record",
        json!({"object_type": "activity", "fields": {"entity_type": "deal", "entity_id": deal.to_string(),
               "activity_type": "note", "content": long_note(1001)}}),
    )
    .await;
    assert!(err, "{out}");
    assert!(
        out["error"]
            .as_str()
            .unwrap_or("")
            .contains("crm.activity.max_chars")
            || out["text"]
                .as_str()
                .unwrap_or("")
                .contains("crm.activity.max_chars"),
        "{out}"
    );
    app.set_setting("crm.activity.max_chars", "200000").await;

    // a proposal page as an MCP image item (the draft: nothing issued yet)
    let (_, doc_number) = proposal(app, deal, "draft", None).await;
    for h in [&full, &act_owner, &read] {
        let (out, images, err) = call(
            app,
            h,
            "render_document_page",
            json!({"type": "sales_document", "id": doc_number, "page": 2}),
        )
        .await;
        assert!(!err && out.get("error").is_none(), "{out}");
        assert_eq!(out["pages"], json!(2));
        assert_eq!(out["which"], json!("draft"));
        assert_eq!(images.len(), 1, "an MCP image item");
        assert_eq!(images[0]["mimeType"], json!("image/jpeg"));
        assert!(images[0]["data"].as_str().unwrap().starts_with("/9j/"));
    }
    let (out, images, _) = call(
        app,
        &full,
        "render_document_page",
        json!({"type": "sales_document", "id": doc_number, "page": 9}),
    )
    .await;
    assert!(images.is_empty());
    assert!(out.to_string().contains("has 2 page(s)"), "{out}");
}

#[tokio::test]
async fn a_proposal_and_its_quotation_go_once_at_full_power() {
    let app = app();
    let _g = app.serial().await;
    granary_services::nary::rhythm::set_working_time_override(Some(true));
    WA_TEXT.lock().unwrap().clear();
    WA_MEDIA.lock().unwrap().clear();
    let sa = owner(app).await;
    let full = token(app, &sa.token, &["full"]).await;
    let cust = customer(app).await;
    let deal = deal_for(app, &cust, None).await;
    let (qid, q_number) = quotation(app, deal, &cust).await;
    let (doc, doc_number) = proposal(app, deal, "draft", Some(qid)).await;
    // 1 Nov 2026: the proposal has no figures; the required-figures gate is not the subject here
    sqlx::query(
        "UPDATE system_settings SET value = '[]' WHERE key = 'proposal.visuals.default_figures'",
    )
    .execute(&mut app.db().await)
    .await
    .unwrap();
    let args = json!({"channel": "whatsapp", "chat_id": cust.chat_id,
                      "text": format!("Dear Sir, please find our proposal {}", u("t")),
                      "attach": [{"type": "sales_document", "id": doc.to_string()},
                                 {"type": "quotation", "id": q_number}]});
    let (first, _, err) = call(app, &full, "propose_customer_message", args.clone()).await;
    assert!(
        !err && first["approval_required"] == json!(true) && first["applied"] == json!(false),
        "{first}"
    );
    let shown = first.to_string();
    assert!(
        shown.contains("from the proposal"),
        "the card lists the bundled quotation: {shown}"
    );
    assert!(shown.contains("already included"), "and warns: {shown}");
    assert!(WA_MEDIA.lock().unwrap().is_empty());
    let (done, _, err) = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": first["apply_id"]}),
    )
    .await;
    assert!(!err && done["applied"] == json!(true), "{done}");
    let media = WA_MEDIA.lock().unwrap().clone();
    let files: Vec<String> = media
        .iter()
        .map(|m| m["filename"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        files.len(),
        2,
        "the proposal and its quotation, once each: {files:?}"
    );
    let q_file = format!("{}.pdf", q_number.replace('/', "-"));
    assert_eq!(
        files.iter().filter(|f| **f == q_file).count(),
        1,
        "{files:?}"
    );
    assert_eq!(
        files
            .iter()
            .filter(|f| f.starts_with(&doc_number.replace('/', "-")))
            .count(),
        1,
        "{files:?}"
    );
    // 3 Nov 2026: both go out with ACME's outbound seal (DocMDP P=2), the stored sealed copy
    let mut db = app.db().await;
    for m in &media {
        let data =
            granary_compat::crypto::pyb64::b64decode_str(m["data"].as_str().unwrap()).unwrap();
        assert_eq!(
            granary_services::pdf_sign::docmdp_permission(&data),
            Some(2),
            "{} is sealed",
            m["filename"]
        );
    }
    let (sealed_key, level): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT sealed_object_key, seal_level FROM document_verifications \
         WHERE entity_type = 'quotation' AND entity_id = $1 AND status = 'active'",
    )
    .bind(qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(level.as_deref(), Some("pades-b-b"));
    let q_media = media
        .iter()
        .find(|m| m["filename"] == json!(q_file))
        .unwrap();
    let stored = STORE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(sealed_key.as_deref().unwrap())
        .cloned()
        .unwrap();
    assert_eq!(
        granary_compat::crypto::pyb64::b64decode_str(q_media["data"].as_str().unwrap()).unwrap(),
        stored
    );
    drop(db);
    assert_eq!(WA_TEXT.lock().unwrap().len(), 1);
    // one send recorded for the quotation
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM domain_events WHERE type = 'quotation.sent' AND entity_id = $1",
    )
    .bind(qid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 1);
    granary_services::nary::rhythm::set_working_time_override(None);
}
