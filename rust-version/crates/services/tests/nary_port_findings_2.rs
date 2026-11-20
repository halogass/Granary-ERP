//! Port of backend/tests/test_nary_port_findings_2.py: bugs the Rust port found in Nary (second batch): an
//! applied action's notifications, records matched on a missing id (IS NULL), the lead's currency, the company
//! LIKE, meeting picks, quotation lines paired by key, man-day rounding.
//!
//! Differences from the Python tests (no monkeypatching in Rust):
//! - 1: Python captures `notify._deliver`; here the caller's notify scope is read (what `deliver_outbox_now`
//!   would hand to `_deliver`).
//! - 3: Python replaces `owner_channel.tell`; here the facts are read from the brief prompt the model is sent
//!   (`facts` is the user message, the same dict `tell` gets).
//! - 5b: the lead is created by an INSERT, not POST /api/v1/crm/leads (the router is wave 4).
//! - 6: Python swaps `clean_company` for one that keeps `_` (the real one strips it, so no wildcard ever reaches
//!   the LIKE); there is no such seam here, so the real cleaner runs and only the outcome is checked.
//! - 8: Python wraps `pricing.price_lines` (adds an add-on line) and `margins.lines_report`; here the pairing
//!   step (`quotes::apply_tier`) gets the same priced keys and report, and `_draft` is run for real to check
//!   that each priced line carries its item's key.

mod nary_n6_common;
mod support;

use chrono::{Duration, Utc};
use granary_db::rows::crm::Deal;
use granary_db::rows::crm_inbox::CrmEmailThread;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::approvals::notify::{self, Notice};
use granary_services::nary::{
    actions, executor, handoff, identity_match, intake, meetings, quotes, rates, replies,
};
use nary_n6_common::*;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

/// `set_session_pool` is process-wide: tests that use it must not overlap (one would read another's
/// database through the shared session pool), so each holds this lock for its whole run.
static SESSION_POOL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A fresh database connection, holding [`SESSION_POOL_LOCK`] until dropped.
struct TestDb {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for TestDb {
    type Target = sqlx::pool::PoolConnection<sqlx::Postgres>;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl std::ops::DerefMut for TestDb {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

async fn db(tag: &str) -> TestDb {
    let guard = SESSION_POOL_LOCK.lock().await;
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    TestDb {
        conn: pool.acquire().await.unwrap(),
        _guard: guard,
    }
}

/// `SimpleNamespace(id=uuid4(), linked_contact_id=..., linked_lead_id=..., linked_deal_id=None)`.
fn thread(contact_id: Option<Uuid>, lead_id: Uuid) -> CrmEmailThread {
    serde_json::from_value(json!({
        "id": Uuid::new_v4(), "created_at": Utc::now(), "updated_at": Utc::now(),
        "linked_contact_id": contact_id, "linked_lead_id": lead_id,
    }))
    .unwrap()
}

async fn lead(db: &mut PgConnection, title: &str, status: &str, currency: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO leads (id, title, source, status, currency, custom, is_deleted) \
         VALUES (gen_random_uuid(), $1, 'email', $2, $3, '{}', false) RETURNING id",
    )
    .bind(title)
    .bind(status)
    .bind(currency)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

// ─── 1. What an applied action notifies goes out ─────────────────────────────

#[tokio::test]
async fn test_what_an_applied_action_notifies_is_delivered_and_a_failed_one_is_not() {
    let mut conn = db("pf2_outbox").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let actor = auth_user(db, me).await;
    actions::register(actions::Action::new(
        "pytest.ping",
        "Ping",
        actions::Schema::strict("Ping", vec![actions::Field::req("text", actions::Ty::Str)]),
        actions::apply_fn(move |db, _actor, p| {
            let text = p["text"].as_str().unwrap_or("").to_string();
            Box::pin(async move {
                let uid = me.to_string();
                notify::notify(
                    db,
                    [uid],
                    Notice::new(&text, "b", "/nary", "pytest", "1")
                        .email(true)
                        .whatsapp(true)
                        .type_("nary_pytest"),
                )
                .await?;
                if text == "fail" {
                    return Err(granary_core::errors::ApiError::http(409, "Refused"));
                }
                Ok(json!({"ok": true}))
            })
        }),
    ));
    let old = set(db, &[("nary.autonomy.levels", r#"{"pytest.ping": 3}"#)]).await;
    let ok = notify::scope(async {
        let card = executor::propose(
            db,
            "pytest.ping",
            json!({"text": "Proposal drafted"}),
            &actor,
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        (card, notify::pending_outbox())
    })
    .await;
    let failed = notify::scope(async {
        let err = executor::propose(
            db,
            "pytest.ping",
            json!({"text": "fail"}),
            &actor,
            executor::ProposeOpts::default(),
        )
        .await;
        (err, notify::pending_outbox())
    })
    .await;
    restore(db, old).await;
    actions::unregister("pytest.ping");
    assert_eq!(ok.0.status, "applied");
    assert!(failed.0.is_err());
    let delivered: Vec<(String, bool, bool)> =
        ok.1.iter()
            .chain(failed.1.iter())
            .map(|d| (d.title.clone(), d.email, d.whatsapp))
            .collect();
    assert_eq!(
        delivered,
        vec![("Proposal drafted".to_string(), true, true)]
    );
}

// ─── 2-4. A missing id is not a match ─────────────────────────────────────────

#[tokio::test]
async fn test_the_handoff_history_of_a_deal_without_a_contact_has_only_its_own_activities() {
    let mut conn = db("pf2_handoff").await;
    let db: &mut PgConnection = &mut conn;
    let deal: Deal = sqlx::query_as(
        "INSERT INTO deals (id, title, stage, currency, custom, is_deleted) \
         VALUES (gen_random_uuid(), $1, 'proposal', 'JPY', '{}', false) RETURNING *",
    )
    .bind(u("Pytest contactless deal"))
    .fetch_one(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, deal_id, activity_type, subject) \
         VALUES (gen_random_uuid(), 'deal', $1, $1, 'note', 'Our scope note')",
    )
    .bind(deal.id)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, activity_type, subject) \
         VALUES (gen_random_uuid(), 'project', gen_random_uuid(), 'note', 'Another customer''s note')",
    )
    .execute(&mut *db)
    .await
    .unwrap();
    let history = handoff::deal_history(db, &deal, 120).await.unwrap();
    assert!(history.contains("Our scope note") && !history.contains("Another customer's note"));
}

#[tokio::test]
async fn test_the_owner_brief_of_a_meeting_without_a_deal_has_only_its_leads_insights() {
    let mut conn = db("pf2_brief").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    brain.set("nary.meeting_brief", Answer::text("1. Brief"));
    let _seam = brain.install().await;
    let cust = customer(db, None, true, "en").await;
    for (lead_id, note) in [(cust.lead_id, "ours"), (None, "someone else's")] {
        sqlx::query(
            "INSERT INTO nary_insights (id, kind, lead_id, content) VALUES (gen_random_uuid(), 'fit_check', $1, $2)",
        )
        .bind(lead_id)
        .bind(json!({"note": note}))
        .execute(&mut *db)
        .await
        .unwrap();
    }
    let meeting: granary_db::rows::nary::NaryMeeting = sqlx::query_as(
        "INSERT INTO nary_meetings (id, lead_id, status, title, slots, starts_at, channel, origin, attendees) \
         VALUES (gen_random_uuid(), $1, 'booked', 'Pytest call', '[]', $2, 'whatsapp', 'nary', '[]') RETURNING *",
    )
    .bind(cust.lead_id)
    .bind(Utc::now() + Duration::minutes(30))
    .fetch_one(&mut *db)
    .await
    .unwrap();
    meetings::owner_brief(db, &meeting).await.unwrap();
    let calls = brain.calls();
    let (_, call) = calls
        .iter()
        .find(|(job, _)| job == "nary.meeting_brief")
        .expect("the brief was asked for");
    let facts: Value = serde_json::from_str(
        call.chat().unwrap().messages[0]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let notes: Vec<&Value> = facts["insights"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &i["content"]["note"])
        .collect();
    assert_eq!(notes, vec![&json!("ours")]);
}

#[tokio::test]
async fn test_an_email_from_a_known_company_never_takes_a_contactless_open_lead() {
    // email_intake._process probes with an unsaved Contact (id None): Lead.contact_id == None matched any
    // open lead without a contact, so a stranger's email was attached to it.
    use granary_services::crm::stages::ACTIVE_LEAD_STATUSES;
    let mut conn = db("pf2_probe").await;
    let db: &mut PgConnection = &mut conn;
    let org: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES (gen_random_uuid(), $1, false, '{}') RETURNING id",
    )
    .bind(u("Pytest Probe Ltd"))
    .fetch_one(&mut *db)
    .await
    .unwrap();
    lead(db, &u("Contactless lead"), ACTIVE_LEAD_STATUSES[0], None).await;
    assert_eq!(
        identity_match::open_opportunity_of(db, None, Some(org))
            .await
            .unwrap(),
        (None, None)
    );
    let mine: Uuid = sqlx::query_scalar(
        "INSERT INTO leads (id, title, source, status, organization_id, custom, is_deleted) \
         VALUES (gen_random_uuid(), $1, 'email', $2, $3, '{}', false) RETURNING id",
    )
    .bind(u("The company's lead"))
    .bind(ACTIVE_LEAD_STATUSES[0])
    .bind(org)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(
        identity_match::open_opportunity_of(db, None, Some(org))
            .await
            .unwrap(),
        (Some(mine), None)
    );
}

// ─── 5. The lead's own currency, else the country's (migration 105) ──────────

#[tokio::test]
async fn test_the_leads_own_currency_wins_over_the_country_default() {
    let mut conn = db("pf2_currency").await;
    let db: &mut PgConnection = &mut conn;
    let jpy = lead(db, &u("JPY lead"), "new", Some("JPY")).await;
    let none = lead(db, &u("No currency"), "new", None).await;
    let stored: Option<String> = sqlx::query_scalar("SELECT currency FROM leads WHERE id = $1")
        .bind(none)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(stored.is_none()); // no USD default any more
    let old = set(
        db,
        &[(
            "nary.intake.currency_by_country",
            r#"{"JP": "JPY", "*": "EUR"}"#,
        )],
    )
    .await;
    let mut got = vec![];
    for (id, expected) in [(jpy, "JPY"), (none, "EUR")] {
        let conv = replies::for_email(db, &thread(None, id), "Hello").await;
        got.push((conv.map(|c| c.currency), expected));
    }
    restore(db, old).await;
    for (currency, expected) in got {
        assert_eq!(currency.unwrap().as_deref(), Some(expected));
    }
}

#[tokio::test]
async fn test_a_manual_lead_without_a_currency_is_quoted_in_the_customers_country_and_usd_stays_usd(
) {
    let mut conn = db("pf2_manual").await;
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, None, true, "en").await; // an English number (its open lead is not used)
    let mut ids = vec![];
    for (name, currency) in [("none", None), ("usd", Some("USD"))] {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO leads (id, title, contact_id, source, status, currency, custom, is_deleted) \
             VALUES (gen_random_uuid(), $1, $2, 'email', 'new', $3, '{}', false) RETURNING id",
        )
        .bind(u(&format!("Manual {name}")))
        .bind(cust.contact_id)
        .bind(currency)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        ids.push(id);
    }
    let old = set(
        db,
        &[(
            "nary.intake.currency_by_country",
            r#"{"JP": "JPY", "*": "USD"}"#,
        )],
    )
    .await;
    let mut got = vec![];
    for (id, expected) in [(ids[0], "JPY"), (ids[1], "USD")] {
        let conv = replies::for_email(db, &thread(Some(cust.contact_id), id), "Hello").await;
        got.push((conv.map(|c| c.currency), expected));
    }
    restore(db, old).await;
    for (currency, expected) in got {
        assert_eq!(currency.unwrap().as_deref(), Some(expected));
    }
}

#[tokio::test]
async fn test_promoting_a_lead_without_a_currency_still_makes_a_usd_deal() {
    let mut conn = db("pf2_promote").await;
    let db: &mut PgConnection = &mut conn;
    let id = lead(db, &u("No currency to promote"), "qualifying", None).await;
    let admin = superadmin(db).await;
    let user = auth_user(db, admin).await;
    let out = granary_services::crm::promote::promote_lead(db, id, &user, "promote")
        .await
        .unwrap();
    let deal_id = Uuid::parse_str(out["deal_id"].as_str().unwrap()).unwrap();
    let currency: Option<String> = sqlx::query_scalar("SELECT currency FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(currency.as_deref(), Some("USD"));
}

// ─── 6. The company name in LIKE ──────────────────────────────────────────────

#[tokio::test]
async fn test_the_company_match_takes_the_name_literally_not_as_a_like_pattern() {
    let mut conn = db("pf2_like").await;
    let db: &mut PgConnection = &mut conn;
    let tag = &Uuid::new_v4().simple().to_string()[..8];
    let decoy: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES (gen_random_uuid(), $1, false, '{}') RETURNING id",
    )
    .bind(format!("z{tag}Xq Pytest")) // "z{tag}_q" as a pattern matches it
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let mut fields = Map::new();
    fields.insert("company".into(), json!(format!("z{tag}_q")));
    fields.insert("name".into(), json!("Bob Pytest"));
    let contact = intake::_contact(db, &fields, None, None).await.unwrap();
    let org_id = contact.organization_id.unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(org_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(org_id != decoy && name == format!("z{tag}_q"));
}

// ─── 7. A meeting pick is 1..len(slots) ───────────────────────────────────────

#[tokio::test]
async fn test_a_pick_of_zero_or_a_bool_books_nothing() {
    let mut conn = db("pf2_pick").await;
    let db: &mut PgConnection = &mut conn;
    let now = Utc::now();
    let slots: Vec<Value> = [1, 2]
        .iter()
        .map(|d| {
            json!({"start": (now + Duration::days(*d)).to_rfc3339(),
                   "end": (now + Duration::days(*d) + Duration::hours(1)).to_rfc3339()})
        })
        .collect();
    let meeting_id: Uuid = sqlx::query_scalar(
        "INSERT INTO nary_meetings (id, status, slots, language, attendee_email, channel, origin, attendees) \
         VALUES (gen_random_uuid(), 'slots_sent', $1, 'en', 'b@example.com', 'whatsapp', 'nary', '[]') RETURNING id",
    )
    .bind(Value::Array(slots.clone()))
    .fetch_one(&mut *db)
    .await
    .unwrap();
    for picked in [0, -1, 3] {
        let err = meetings::book(db, meeting_id, picked).await.unwrap_err();
        assert_eq!(err.status().as_u16(), 400, "{picked}");
    }
    let status: String = sqlx::query_scalar("SELECT status FROM nary_meetings WHERE id = $1")
        .bind(meeting_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(status, "slots_sent");
    // True was read as 1
    assert!(actions::get("meeting.book")
        .unwrap()
        .validate(&json!({"meeting_id": meeting_id.to_string(), "picked": true}))
        .is_err());
    assert!(
        meetings::valid_pick(&json!(2), slots.len())
            && !meetings::valid_pick(&json!(true), slots.len())
            && !meetings::valid_pick(&json!(0), slots.len())
    );
}

// ─── 8. Quotation lines and the pricing report, by key ───────────────────────

#[tokio::test]
async fn test_the_tier_discount_goes_to_the_line_it_was_checked_for() {
    // A priced line the items don't have (an add-on) shifted the report: each item took its neighbour's floor.
    let (a_id, b_id) = (Uuid::new_v4(), Uuid::new_v4());
    let mut items = vec![
        json!({"product_id": a_id.to_string(), "quantity": "1", "discount_percentage": "0", "is_optional": false}),
        json!({"product_id": b_id.to_string(), "quantity": "1", "discount_percentage": "0", "is_optional": false}),
    ];
    // The priced lines with the add-on first, and the report that follows them.
    let keys = vec!["addon".to_string(), "0".to_string(), "1".to_string()];
    let report = vec![
        json!({"name": "Pytest add-on", "max_discount_pct": "0"}),
        json!({"name": "Pytest Floor Box", "max_discount_pct": "3"}),
        json!({"name": "Pytest Free Box", "max_discount_pct": null}),
    ];
    let clipped = quotes::apply_tier(&mut items, &keys, &report, Decimal::from(10)).unwrap();
    let by_product: Vec<(String, String)> = items
        .iter()
        .map(|i| {
            (
                i["product_id"].as_str().unwrap().to_string(),
                i["discount_percentage"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        by_product,
        vec![
            (a_id.to_string(), "3".to_string()),
            (b_id.to_string(), "10".to_string())
        ]
    );
    assert_eq!(clipped, vec!["Pytest Floor Box: 3% (the floor)"]);

    // _draft itself: every priced line carries the key of the item it came from.
    let mut conn = db("pf2_quote_keys").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    owner(db).await;
    let (_, sku_a) = priced_product(db, "Pytest Floor Box", 10).await;
    let (_, sku_b) = priced_product(db, "Pytest Free Box", 10).await;
    let old = set(
        db,
        &[(
            "nary.price.catalog_terms",
            &json!({"laptop": ["Pytest Floor Box", "Pytest Free Box"]}).to_string(),
        )],
    )
    .await;
    brain.set(
        "nary.quote_lines",
        Answer::json(json!({"subject": "Quotation", "unclear": null, "questions": [], "lines": [
            {"sku": sku_a, "quantity": 1, "optional": false}, {"sku": sku_b, "quantity": 1, "optional": false}]})),
    );
    let mut conv = replies::Conversation::new("whatsapp", "Two boxes please");
    conv.currency = Some("JPY".into());
    let d = quotes::_draft(
        db,
        &conv,
        Some("laptop"),
        None,
        None,
        Some(Decimal::from(10)),
    )
    .await;
    restore(db, old).await;
    let d = d.unwrap().expect("a draft");
    let keys: Vec<&str> = d.res.lines.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(keys, vec!["0", "1"]);
    assert_eq!(d.items.len(), 2);
}

// ─── 10. Man-day rates to the JPY 1.000 ────────────────────────────────────────

#[tokio::test]
async fn test_a_man_day_rate_is_rounded_to_thousands() {
    // quantize(Decimal("1000")) rounded to whole yen only. Half-even at the half-thousand, as the code rounds.
    let mut conn = db("pf2_man_day").await;
    let db: &mut PgConnection = &mut conn;
    let old = set(
        db,
        &[
            ("proposal.cost.load_factor", "1"),
            ("proposal.cost.days_per_month", "1"),
            ("proposal.cost.min_monthly", "0"),
        ],
    )
    .await;
    let mut got = vec![];
    for (monthly, expected) in [
        ("1234567", "1235000"),
        ("1234499.99", "1234000"),
        ("1234500", "1234000"),
        ("1235500", "1236000"),
        ("1234500.01", "1235000"),
        ("1000000", "1000000"),
    ] {
        let out = rates::man_day(db, monthly.parse().unwrap()).await;
        got.push((monthly, out.map(|d| d.to_string()), expected));
    }
    restore(db, old).await;
    for (monthly, out, expected) in got {
        assert_eq!(out.unwrap(), expected, "{monthly}");
    }
}
