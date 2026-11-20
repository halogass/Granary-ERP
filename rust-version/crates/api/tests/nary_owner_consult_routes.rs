//! The quotation consult through the real app (tests/test_nary_owner_channel.py, the two tests that need
//! POST /api/v1/nary/owner/topics/{id}/answer):
//! - test_a_quotation_request_is_a_consult_and_the_ceos_answer_drafts_it_with_the_tier;
//! - test_a_customer_asking_again_is_not_a_second_consult.
//!
//! The P3 harness (tests/nary_p3_support.py): the registry's scripted brain (`Brain::p2`), a wiremock
//! WhatsApp service (`wa`), `rhythm::set_working_time_override` (`working_hours`), the n6 builders
//! (customer, deal_for, cards, score_answer, settings). `ingest` and `dispatch` run on the server's runtime
//! (Nary's session pool lives there, as the app's pool does).

#[path = "../../services/tests/nary_n6_common/mod.rs"]
mod nary_n6_common;
mod support;

use granary_db::rows::nary::NaryOwnerTopic;
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use granary_services::nary::{identity, owner_channel, rhythm};
use nary_n6_common::{cards, customer, deal_for, restore, score_answer, set, u, Cust};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{TestApp, TestUser};
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TAG: &str = "nary_owner_consult";

/// The `wa` fixture: /send answers with a message id, /contact/ with a plain profile, anything else 404.
struct Wa;

impl Respond for Wa {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path();
        if req.method.as_str() == "POST" && path == "/send" {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let to = body["to"].as_str().unwrap_or("").to_string();
            return ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
                "messageId": format!("true_{to}_{}", Uuid::new_v4().simple().to_string()[..16].to_uppercase())}));
        }
        if path.starts_with("/contact/") {
            return ResponseTemplate::new(200).set_body_json(
                json!({"isMyContact": false, "isBusiness": false, "pushname": "Bob"}),
            );
        }
        ResponseTemplate::new(404).set_body_json(json!({"error": "no route"}))
    }
}

static WA: Lazy<String> = Lazy::new(|| {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(Wa)
                .mount(&server)
                .await;
            tx.send(server.uri()).unwrap();
            std::future::pending::<()>().await;
        });
    });
    let uri = rx.recv().unwrap();
    std::env::set_var("WHATSAPP_SERVICE_URL", &uri);
    std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
    uri
});

/// The app, with the fake WhatsApp service in the environment before it starts.
fn app() -> &'static TestApp {
    Lazy::force(&WA);
    support::app(TAG)
}

/// `_p2_clean` / `_p3_clean` / `_no_topics`: Nary's queue, cards, chats and topics of earlier tests.
async fn wipe(db: &mut PgConnection) {
    for sql in [
        "DELETE FROM nary_owner_topics",
        "DELETE FROM agent_steps",
        "DELETE FROM agent_pending_actions",
        "DELETE FROM agent_runs",
        "DELETE FROM domain_events",
        "DELETE FROM nary_triage_verdicts",
        "DELETE FROM nary_expected_arrivals",
        "DELETE FROM nary_scores",
        "DELETE FROM nary_cadences",
        "DELETE FROM nary_insights",
        "DELETE FROM wa_sent_messages",
        "DELETE FROM wa_messages",
        "DELETE FROM wa_chats",
        "DELETE FROM staff_whatsapp_links",
        "DELETE FROM nary_memory WHERE source_ref IS NOT NULL",
        "DELETE FROM nary_identity_links",
        "DELETE FROM wa_import_chats",
        "DELETE FROM system_settings WHERE category = 'nary_state'",
    ] {
        sqlx::query(sql)
            .execute(&mut *db)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

/// The fixtures of both tests (`client, brain, wa, working_hours`), then `_owner_with_chat()`: the pytest
/// superadmin (the client's user) is the Owner, with a primary linked WhatsApp chat.
async fn setup(app: &TestApp, brain: &Brain) -> (PgConnection, Seam, TestUser) {
    let seam = brain.install().await;
    let mut db = app.db().await;
    wipe(&mut db).await;
    // the reply comes at once here (fix batch 1's debounce is tested in nary_fix1_replies.rs)
    sqlx::query("UPDATE system_settings SET value = '0' WHERE key = 'nary.reply.debounce_seconds'")
        .execute(&mut db)
        .await
        .unwrap();
    rhythm::set_working_time_override(Some(true));
    let sa = app.superadmin().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    let cid = format!(
        "8190{}{:07}@c.us",
        1 + Uuid::new_v4().as_u128() % 9,
        Uuid::new_v4().as_u128() % 10_000_000
    );
    let mut link = identity::link_chat(&mut db, sa.id, Some(&cid), Some("Owner phone"), None)
        .await
        .unwrap();
    identity::set_primary(&mut db, &mut link).await.unwrap();
    assert_eq!(
        owner_channel::owner(&mut db).await.unwrap().1.as_deref(),
        Some(cid.as_str())
    );
    (db, seam, sa)
}

fn teardown() {
    rhythm::set_working_time_override(None);
}

/// `priced_product(name=..., jpy=15_000_000, usd=1000, cost=..., family="laptops", tiers=...)`: the sku.
async fn priced_product(
    db: &mut PgConnection,
    name: &str,
    cost: Option<i64>,
    tiers: &[(i64, Option<i64>, i64)],
) -> String {
    let id = Uuid::new_v4();
    let sku = u("PYT");
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, family, is_active, is_orderable, \
         description, pricing_mode, sale_model, price_metric, modules, addon_for, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, 'physical', 'Unit', $4, 'laptops', true, true, \
         'Laptop with 16 GB memory, 1 year warranty', 'list', 'one_time', 'unit', '{}', '{}', false, false)",
    )
    .bind(id)
    .bind(&sku)
    .bind(name)
    .bind(cost.map(rust_decimal::Decimal::from))
    .execute(&mut *db)
    .await
    .unwrap();
    for (cur, amount) in [("JPY", 15_000_000i64), ("USD", 1000)] {
        sqlx::query(
            "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, $3, $4, '2000-01-01')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(cur)
        .bind(rust_decimal::Decimal::from(amount))
        .execute(&mut *db)
        .await
        .unwrap();
    }
    for (lo, hi, pct) in tiers {
        sqlx::query(
            "INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(*lo as i32)
        .bind(hi.map(|h| h as i32))
        .bind(rust_decimal::Decimal::from(*pct))
        .execute(&mut *db)
        .await
        .unwrap();
    }
    sku
}

/// `payload(chat, body)`: an inbound text from the customer.
fn payload(chat: &str, body: &str) -> Value {
    let key = Uuid::new_v4().simple().to_string()[..20].to_uppercase();
    let name = format!("Bob Pytest{}", &Uuid::new_v4().simple().to_string()[..6]);
    json!({"messageId": format!("false_{chat}_{key}"), "chatId": chat, "from": chat, "to": "818000@c.us",
        "body": body, "type": "chat", "timestamp": chrono::Utc::now().timestamp(), "hasMedia": false,
        "fromMe": false, "ack": 1, "chatName": name, "isGroup": false, "fromName": name, "isMyContact": false})
}

/// `ingest(p)`: in its own session, committed (on the server's runtime).
async fn ingest(app: &TestApp, p: Value) -> Value {
    app.on_server(async move {
        let pool = granary_services::nary::session_pool().unwrap();
        let mut tx = pool.begin().await.unwrap();
        let out = granary_services::nary::wa_ingest::ingest(&mut tx, &p, "live")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        out
    })
    .await
}

/// `dispatch()`: Nary's event dispatcher until the queue is empty.
async fn dispatch(app: &TestApp) -> (i64, i64) {
    app.on_server(async {
        let (mut d, mut f) = (0, 0);
        for _ in 0..10 {
            let out = granary_services::nary::events::dispatch_pending(None)
                .await
                .unwrap();
            let od = out.get("dispatched").and_then(Value::as_i64).unwrap_or(0);
            let of = out.get("failed").and_then(Value::as_i64).unwrap_or(0);
            d += od;
            f += of;
            if od == 0 && of == 0 {
                break;
            }
        }
        (d, f)
    })
    .await
}

async fn consult_topics(db: &mut PgConnection) -> Vec<NaryOwnerTopic> {
    sqlx::query_as("SELECT * FROM nary_owner_topics WHERE kind = 'quotation_consult'")
        .fetch_all(&mut *db)
        .await
        .unwrap()
}

async fn topic(db: &mut PgConnection, id: Uuid) -> NaryOwnerTopic {
    sqlx::query_as("SELECT * FROM nary_owner_topics WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn set_waiting(db: &mut PgConnection, id: Uuid) {
    sqlx::query("UPDATE nary_owner_topics SET status = 'waiting' WHERE id = $1")
        .bind(id)
        .execute(&mut *db)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_a_quotation_request_is_a_consult_and_the_ceos_answer_drafts_it_with_the_tier() {
    let app = app();
    let _serial = app.serial().await;
    let brain = Brain::p2();
    let (mut db, _seam, sa) = setup(app, &brain).await;
    let cust: Cust = customer(&mut db, None, true, "en").await;
    deal_for(&mut db, &cust, "proposal").await;
    let sku = priced_product(
        &mut db,
        "Pytest Consult Box",
        Some(9_000_000),
        &[(1, None, 10)],
    )
    .await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(3, 3, 3, 2, "quotation", "laptops")),
    );
    brain.set("nary.quote_lines", Answer::json(json!({"subject": "Laptops for 3 offices", "lines": [
        {"sku": sku, "quantity": 3, "optional": false}], "unclear": null,
        "questions": [{"question": "Which sites?", "who": "customer"}, {"question": "Offer the tier?", "who": "owner"}]})));
    let terms = json!({"laptops": ["Pytest Consult Box"], "pricing_vague": ["Pytest Consult Box"]})
        .to_string();
    // the seed's margin bands name no families: the laptops family is in the products band (floor 35)
    let margins = json!({"products": {"label": "Products", "target": "40", "floor": "35", "families": ["laptops"]},
                         "services": {"label": "Services", "target": "45", "floor": "35", "families": []}})
    .to_string();
    let old = set(
        &mut db,
        &[
            ("nary.price.catalog_terms", &terms),
            ("sales.margins", &margins),
        ],
    )
    .await;

    // _consult_flow
    let chat = cust.chat_id.clone().unwrap();
    ingest(app, payload(&chat, "Please send a quotation for 3 units.")).await;
    dispatch(app).await;
    let drafted_before = cards(&mut db, "quotation.create_draft").await;
    let topics = consult_topics(&mut db).await;
    let calls = brain.calls();
    let jobs = brain.jobs();
    let mut r = None;
    if let [t] = topics.as_slice() {
        set_waiting(&mut db, t.id).await; // as if sent: now it waits for the answer
        r = Some(
            app.post(
                &sa.token,
                &format!("/api/v1/nary/owner/topics/{}/answer", t.id),
                json!({"answer": "Go ahead, offer the tier", "tier_pct": 10}),
            )
            .await,
        );
    }
    restore(&mut db, old).await;
    teardown();

    assert!(
        drafted_before.is_empty(),
        "nothing drafted before the Owner"
    );
    assert_eq!(topics.len(), 1, "{jobs:?}");
    let t = &topics[0];
    let line = &t.facts.as_ref().unwrap()["lines"][0];
    assert!(
        line["margin_pct"] == "40.00"
            && !line["floor_pct"].is_null()
            && line["max_discount_pct"] == "7.69",
        "{line}"
    );
    assert!(
        t.text.contains("Offer the tier?") && !t.text.contains("Which sites?"),
        "{}",
        t.text
    );
    let at = jobs
        .iter()
        .position(|j| j == "nary.reply_draft")
        .expect("a reply was drafted");
    assert!(
        calls[at].1.system().contains("Which sites?"),
        "the customer is asked what they know"
    );
    let r = r.unwrap();
    assert_eq!(r.status, 200, "{}", r.text());
    let card = cards(&mut db, "quotation.create_draft")
        .await
        .pop()
        .unwrap();
    assert_eq!(card.payload["items"][0]["discount_percentage"], "7.69"); // 10% asked, held at the 35% floor
    assert!(
        card.payload["internal_notes"]
            .as_str()
            .unwrap()
            .contains("Owner: Go ahead")
            && card.rationale.as_deref().unwrap_or("").contains("floor"),
        "{:?} {:?}",
        card.payload["internal_notes"],
        card.rationale
    );
    assert_eq!(topic(&mut db, t.id).await.status, "answered");
}

#[tokio::test]
async fn test_a_customer_asking_again_is_not_a_second_consult() {
    let app = app();
    let _serial = app.serial().await;
    let brain = Brain::p2();
    let (mut db, _seam, sa) = setup(app, &brain).await;
    let cust: Cust = customer(&mut db, None, true, "en").await;
    deal_for(&mut db, &cust, "proposal").await;
    let sku = priced_product(&mut db, "Pytest Twice Box", None, &[(5, None, 5)]).await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(3, 3, 3, 2, "quotation", "laptops")),
    );
    brain.set(
        "nary.quote_lines",
        Answer::json(
            json!({"subject": "Twice", "lines": [{"sku": sku, "quantity": 2, "optional": false}],
            "unclear": null, "questions": []}),
        ),
    );
    let terms =
        json!({"laptops": ["Pytest Twice Box"], "pricing_vague": ["Pytest Twice Box"]}).to_string();
    let old = set(&mut db, &[("nary.price.catalog_terms", &terms)]).await;
    let chat = cust.chat_id.clone().unwrap();
    ingest(app, payload(&chat, "Send a quotation for 2 units please")).await;
    dispatch(app).await;
    let first = consult_topics(&mut db).await;
    let jobs = brain.jobs();
    let mut r = None;
    if let [t] = first.as_slice() {
        set_waiting(&mut db, t.id).await;
        r = Some(
            app.post(
                &sa.token,
                &format!("/api/v1/nary/owner/topics/{}/answer", t.id),
                json!({"answer": "Go ahead"}),
            )
            .await,
        );
        if r.as_ref().is_some_and(|r| r.status == 200) {
            ingest(app, payload(&chat, "Still waiting for the quotation")).await;
            dispatch(app).await;
        }
    }
    restore(&mut db, old).await;
    teardown();

    assert_eq!(first.len(), 1, "{jobs:?}");
    assert!(
        first[0].text.contains("margin is unknown"),
        "no cost on file: said, not hidden: {}",
        first[0].text
    );
    let r = r.unwrap();
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(consult_topics(&mut db).await.len(), 1);
}
