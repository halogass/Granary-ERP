//! The model-calling parts of nary/{identity_match, scoring}: the request each call sends (system prompt,
//! messages, response schema) and how each reply is handled, against compat-tests/golden/nary_identity/
//! requests.json (recorded from the REAL backend with a scripted AI); then ports of
//! tests/test_nary_p3_playbooks.py (scoring) and tests/test_nary_p2_identity.py (the tiers) on the brain seam.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use granary_db::rows::crm::Lead;
use granary_db::rows::nary::WaChat;
use granary_services::ai::registry::seam::{AiCall, Answer, Brain};
use granary_services::nary::{identity_match as im, owner_channel, rhythm, scoring};
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

fn golden(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compat-tests/golden/nary_identity")
                .join(name),
        )
        .unwrap(),
    )
    .unwrap()
}

fn answer(v: &Value) -> Answer {
    match v {
        Value::String(s) => Answer::text(s.clone()),
        other => Answer::json(other.clone()),
    }
}

fn brain_for(answers: &Value) -> Brain {
    let brain = Brain::new();
    for (job, a) in answers.as_object().unwrap() {
        brain.set(job, answer(a));
    }
    brain
}

fn calls_json(brain: &Brain) -> Value {
    brain
        .calls()
        .iter()
        .map(|(job, call)| {
            let c = call.chat().unwrap();
            json!({"job": job, "system": c.system, "messages": c.messages, "response_schema": c.response_schema})
        })
        .collect()
}

async fn chat_row(db: &mut PgConnection, chat_id: &str) -> WaChat {
    sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn requests_golden() {
    let c = golden("candidates.json");
    let r = golden("requests.json");
    let pool = support::fresh_db("n8_identity_requests").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    for sql in c["scenario"].as_array().unwrap() {
        sqlx::query(sql.as_str().unwrap())
            .execute(&mut *db)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, status, is_my_contact, attribution_asked, business_profile) \
         VALUES ($1, 'req@c.us', '+8181222222222', 'Harper “Mr”', 'active', false, false, \
         '{\"description\": \"Shop ü\", \"rating\": 4.5, \"website\": [\"x.example\"]}')",
    )
    .bind(Uuid::parse_str("88000000-0000-0000-0000-00000000005a").unwrap())
    .execute(&mut *db)
    .await
    .unwrap();
    let chat = chat_row(db, "req@c.us").await;
    let cfg = im::settings_bundle(db).await.unwrap();
    let text_in = format!(
        "Name: Harper Wilson\nCompany: Northwind Traders\nI would like to continue the laptop discussion ✓ {}",
        "x".repeat(3100)
    );
    let cands = im::candidates_for_chat(db, &chat, Some(&text_in), &cfg, None)
        .await
        .unwrap();

    // GOLDEN_BLESS=1 writes the actual outputs and requests back to requests.json (Python can no longer
    // record them); check the diff before keeping it.
    let bless = bless::blessing();
    let mut blessed: Vec<Value> = Vec::new();
    for case in r["requests"].as_array().unwrap() {
        let mut new_case = case.clone();
        let brain = brain_for(&case["answers"]);
        let seam = brain.install().await;
        let what = format!("{} {}", case["fn"], case["case"]);
        match case["fn"].as_str().unwrap() {
            "review_likely" => {
                let (same, reason) = im::review_likely(db, &chat, Some(&text_in), &cands[0], None)
                    .await
                    .unwrap();
                new_case["out"] = json!([same, reason]);
                if !bless {
                    assert_eq!(json!([same, reason]), case["out"], "{what}");
                }
            }
            "ai_hints" => {
                let h = im::ai_hints(db, &chat, Some(&"y".repeat(4100)), &cfg, None)
                    .await
                    .unwrap();
                let got = h.map(|h| {
                    json!({"name": h.name, "company": h.company, "product": h.product, "emails": h.emails,
                           "phones": h.phones})
                });
                let got = got.unwrap_or(Value::Null);
                new_case["out"] = got.clone();
                if !bless {
                    assert_eq!(got, case["out"], "{what}");
                }
            }
            "score" => {
                let mut tx = db.begin().await.unwrap();
                let mut lead: Lead = sqlx::query_as(
                    "INSERT INTO leads (id, title, source, status, custom, ai_score, is_deleted) \
                     VALUES ($1, 'Golden lead', 'whatsapp_direct', 'qualifying', '{}', '{\"keep\": 1}', false) RETURNING *",
                )
                .bind(Uuid::new_v4())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                let transcript = format!("We need 10 laptops ✓ {}", "t".repeat(8100));
                let row = scoring::score(
                    &mut tx,
                    &transcript,
                    Some(&mut lead),
                    None,
                    Some("req@c.us"),
                    None,
                    "CTX line",
                )
                .await
                .unwrap()
                .unwrap();
                tx.rollback().await.unwrap();
                let got = json!({"total": row.total, "class": row.class,
                                 "parts": [row.budget, row.authority, row.need, row.timeline],
                                 "red_flags": row.red_flags, "green_flags": row.green_flags, "rationale": row.rationale,
                                 "intent": row.intent, "product": row.product, "model": row.model,
                                 "cascaded": row.cascaded, "ai_score": lead.ai_score});
                // `_model` is the stub's model_used: the Python recorder answered "stub-model", the brain answers
                // "stub:<job>"; everything else must match.
                let mut got = got;
                let mut want = case["out"].clone();
                assert!(got["model"].as_str().unwrap().starts_with("stub"), "{what}");
                let mut kept = got.clone();
                kept["model"] = want["model"].clone();
                new_case["out"] = kept;
                got["model"] = Value::Null;
                want["model"] = Value::Null;
                if !bless {
                    assert_eq!(got, want, "{what}");
                }
            }
            other => panic!("{other}"),
        }
        let calls = calls_json(&brain);
        new_case["calls"] = calls.clone();
        blessed.push(new_case);
        drop(seam);
        if bless {
            continue;
        }
        assert_eq!(calls, case["calls"], "{what}");
        // The request bodies byte for byte (key order included).
        assert_eq!(calls.to_string(), case["calls"].to_string(), "{what}");
    }
    if bless {
        bless::write("nary_identity/requests.json", &json!({"requests": blessed}));
    }
}

fn score_answer(b: i64, a: i64, n: i64, t: i64, intent: &str) -> Value {
    json!({"budget": b, "authority": a, "need": n, "timeline": t, "red_flags": [], "green_flags": [],
           "disqualify": false, "rationale": "pytest", "intent": intent, "product": "laptop"})
}

async fn score_lead(db: &mut PgConnection) -> (Option<granary_db::rows::nary::NaryScore>, Uuid) {
    let mut lead: Lead = sqlx::query_as(
        "INSERT INTO leads (id, title, source, status, custom, is_deleted) \
         VALUES ($1, 'Pytest lead', 'whatsapp_direct', 'qualifying', '{}', false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let row = scoring::score(
        db,
        "We need 10 laptops next month, the budget is approved",
        Some(&mut lead),
        None,
        None,
        None,
        "",
    )
    .await
    .unwrap();
    (row, lead.id)
}

async fn make_owner(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Owner', 'x', true, true, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{id}@granary.test"))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn hot(db: &mut PgConnection, me: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND title LIKE 'Hot lead%'",
    )
    .bind(me)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

#[tokio::test]
async fn scoring_playbook() {
    let pool = support::fresh_db("n8_scoring").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = make_owner(db).await;

    // test_a_clear_score_stays_on_haiku
    let brain = Brain::p2();
    let seam = brain.install().await;
    rhythm::set_working_time_override(Some(true));
    brain.set(
        "nary.score",
        Answer::json(score_answer(3, 2, 3, 0, "question")),
    ); // 8: warm, two away from an edge
    let (row, lead_id) = score_lead(db).await;
    let row = row.unwrap();
    assert_eq!(
        (row.total, row.class.as_str(), row.cascaded),
        (8, "warm", false)
    );
    assert_eq!(brain.jobs(), vec!["nary.score"]);
    let ai: Value = sqlx::query_scalar("SELECT ai_score FROM leads WHERE id = $1")
        .bind(lead_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(ai["temperature"] == "warm" && ai["bant"]["need"] == 3);
    drop(seam);

    // test_a_score_at_a_class_edge_goes_to_sonnet
    let brain = Brain::p2();
    let seam = brain.install().await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(3, 3, 3, 0, "question")),
    ); // 9: one below hot
    brain.set(
        "nary.cascade_review",
        Answer::json(score_answer(3, 3, 3, 1, "price")),
    );
    owner_channel::set_enabled_override(Some(false));
    let (row, _) = score_lead(db).await;
    let row = row.unwrap();
    assert_eq!(brain.jobs(), vec!["nary.score", "nary.cascade_review"]);
    assert_eq!(
        (
            row.total,
            row.class.as_str(),
            row.cascaded,
            row.intent.as_deref()
        ),
        (10, "hot", true, Some("price"))
    );
    drop(seam);

    // test_a_hot_lead_interrupts_the_owner_in_working_hours_only
    sqlx::query("DELETE FROM notifications WHERE user_id = $1")
        .bind(me)
        .execute(&mut *db)
        .await
        .unwrap();
    let brain = Brain::p2();
    let seam = brain.install().await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(3, 3, 3, 3, "question")),
    );
    rhythm::set_working_time_override(Some(false));
    score_lead(db).await;
    assert_eq!(hot(db, me).await, 0);
    rhythm::set_working_time_override(Some(true));
    score_lead(db).await;
    assert!(hot(db, me).await >= 1);
    drop(seam);
    rhythm::set_working_time_override(None);
    owner_channel::set_enabled_override(None);
}

#[tokio::test]
async fn identity_tiers() {
    let pool = support::fresh_db("n8_identity_tiers").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    async fn add_chat(
        db: &mut PgConnection,
        (chat_id, phone, name): (&str, Option<&str>, &str),
    ) -> WaChat {
        sqlx::query_as(
            "INSERT INTO wa_chats (id, chat_id, phone, display_name, status, is_my_contact, attribution_asked) \
             VALUES ($1, $2, $3, $4, 'active', false, false) RETURNING *",
        )
        .bind(Uuid::new_v4())
        .bind(chat_id)
        .bind(phone)
        .bind(name)
        .fetch_one(&mut *db)
        .await
        .unwrap()
    }

    // test_certain_the_chat_comes_from_the_number_a_whatsapp_form_gave
    let brain = Brain::p2();
    let seam = brain.install().await;
    let (form_lead, arrival) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO leads (id, title, source, status, custom, is_deleted) VALUES ($1, 'Form lead', 'whatsapp', 'new', '{}', false)")
        .bind(form_lead).execute(&mut *db).await.unwrap();
    sqlx::query(
        "INSERT INTO nary_expected_arrivals (id, lead_id, phone, name, company, message, due_at, status) \
         VALUES ($1, $2, '+8181200001111', 'Dave Stark', 'STARKSAFE', 'laptop order', now() + interval '2 hours', 'pending')",
    )
    .bind(arrival).bind(form_lead).execute(&mut *db).await.unwrap();
    let mut chat = add_chat(db, ("8181200001111@c.us", Some("+8181200001111"), "Dave")).await;
    let out = im::resolve_chat(
        db,
        &mut chat,
        Some("Hello, I filled your form about a laptop order"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(out["tier"], "certain");
    assert_eq!(
        chat_row(db, "8181200001111@c.us").await.lead_id,
        Some(form_lead)
    );
    let a: (String, Option<String>) =
        sqlx::query_as("SELECT status, chat_id FROM nary_expected_arrivals WHERE id = $1")
            .bind(arrival)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(a, ("arrived".into(), Some("8181200001111@c.us".into())));
    assert!(brain.jobs().is_empty()); // certain from the start: no model call
    drop(seam);

    // test_likely_name_and_company_is_reviewed_then_linked_with_an_undo
    let brain = Brain::p2();
    let seam = brain.install().await;
    let (org, contact) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO organizations (id, name, custom, is_deleted) VALUES ($1, 'Wayne Enterprises Inc', '{}', false)")
        .bind(org).execute(&mut *db).await.unwrap();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, organization_id, custom, is_deleted) VALUES ($1, 'Harper', 'Lane', $2, '{}', false)")
        .bind(contact).bind(org).execute(&mut *db).await.unwrap();
    brain.set(
        "nary.cascade_review",
        Answer::json(json!({"same": true, "reason": "same name and company"})),
    );
    let mut chat = add_chat(db, ("8181200002222@c.us", Some("+8181200002222"), "Harper")).await;
    let out = im::resolve_chat(
        db,
        &mut chat,
        Some("Name: Harper Lane\nCompany: Wayne Enterprises\nI would like to continue the laptop discussion"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(out["tier"], "likely");
    assert_eq!(
        brain.jobs(),
        vec!["nary.identity_match", "nary.cascade_review"]
    );
    assert_eq!(
        chat_row(db, "8181200002222@c.us").await.contact_id,
        Some(contact)
    );
    let link: granary_db::rows::nary::NaryIdentityLink = sqlx::query_as(
        "SELECT * FROM nary_identity_links WHERE subject_ref = '8181200002222@c.us' AND status = 'linked'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(
        link.tier == "likely"
            && link.cascaded
            && link.review.as_deref() == Some("same name and company")
    );
    let owner = make_owner(db).await;
    im::undo(db, link.id, owner).await.unwrap();
    assert_ne!(
        chat_row(db, "8181200002222@c.us").await.contact_id,
        Some(contact)
    );
    drop(seam);

    // test_weak_a_name_alone_becomes_a_pending_link (the card itself is the channels' work)
    let brain = Brain::p2();
    let seam = brain.install().await;
    let rinjani = Uuid::new_v4();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, custom, is_deleted) VALUES ($1, 'Rachel', 'Greenwood', '{}', false)")
        .bind(rinjani).execute(&mut *db).await.unwrap();
    let mut chat = add_chat(
        db,
        (
            "8181200003333@c.us",
            Some("+8181200003333"),
            "Rachel Greenwood",
        ),
    )
    .await;
    let out = im::resolve_chat(
        db,
        &mut chat,
        Some("Hello, another question about the support plan"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(out["tier"], "weak");
    assert_eq!(
        (out["kind"].clone(), out["target_id"].clone()),
        (json!("contact"), json!(rinjani.to_string()))
    );
    assert_ne!(
        chat_row(db, "8181200003333@c.us").await.contact_id,
        Some(rinjani)
    );
    let status: String = sqlx::query_scalar(
        "SELECT status FROM nary_identity_links WHERE subject_ref = '8181200003333@c.us'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(status, "pending");
    drop(seam);

    // likely, but the review says no: weak, with the review's reason
    let brain = Brain::p2();
    let seam = brain.install().await;
    brain.set(
        "nary.cascade_review",
        Answer::json(json!({"same": false, "reason": "different company"})),
    );
    let mut chat = add_chat(db, ("8181200004444@c.us", Some("+8181200004444"), "Harper")).await;
    let out = im::resolve_chat(
        db,
        &mut chat,
        Some("Name: Harper Lane\nCompany: Wayne Enterprises"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(out["tier"], "weak");
    let review: Option<String> = sqlx::query_scalar(
        "SELECT review FROM nary_identity_links WHERE subject_ref = '8181200004444@c.us'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(review.as_deref(), Some("different company"));
    drop(seam);

    // nobody: no tier, nothing written
    let brain = Brain::p2();
    let seam = brain.install().await;
    let mut chat = add_chat(db, ("8181200005555@c.us", Some("+8181200005555"), "Zzyzx")).await;
    let out = im::resolve_chat(db, &mut chat, Some("hi"), None)
        .await
        .unwrap();
    assert_eq!(out, json!({"tier": null}));
    assert!(brain.jobs().is_empty()); // under 20 characters: no hints call
    drop(seam);
    let _ = AiCall::chat;
}
