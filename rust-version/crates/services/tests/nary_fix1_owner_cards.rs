//! Nary fix batch 1, agent N3 (19 Oct 2026 review, items C4-C14): the Owner's chat, cards and briefs.
//!
//! - C4  an apply ("1") is applied as shown: a topic notice in between no longer makes the number stale, and
//!   an apply with edits the Owner never named is applied without them;
//! - C5-C8 a card shows the whole message, the prices, the quotation lines and what tells two merge copies apart;
//!   "2" prints the whole card;
//! - C9  a customer asking for a quotation urgently goes to the top of the brief, worded as what they want;
//! - C10 after an outage only the newest brief of each kind goes, repeated alerts become one, problems with
//!   the same title are one line with a count and a description; several due slots send only the latest;
//! - C11 an overdue follow-up is listed once with "still open since" and done / snooze / drop;
//! - C12 a write-up with its numbers out of order falls back to the plain numbered list;
//! - C13 "N more after this" counts the topics really waiting (each card once, decided ones gone);
//! - C14 inline_* variants of the website form sources are known sources.

mod nary_owner_common;

use chrono::{TimeDelta, Utc};
use nary_owner_common as c;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_services::ai::base::AiError;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::cards::{self, CardView};
use granary_services::nary::{briefs, commands, executor, owner_channel, rhythm, web3forms};

const TAG: &str = "n3_fix1_owner_cards";

async fn wipe(db: &mut PgConnection) {
    for sql in [
        "DELETE FROM nary_owner_topics",
        "DELETE FROM agent_pending_actions",
        "DELETE FROM notifications WHERE type LIKE 'nary_%'",
        "DELETE FROM system_settings WHERE category = 'nary_state'",
        "DELETE FROM crm_reminders",
    ] {
        sqlx::query(sql).execute(&mut *db).await.unwrap();
    }
}

async fn wa_chat_row(
    db: &mut PgConnection,
    cid: &str,
    staff: Option<Uuid>,
) -> granary_db::rows::nary::WaChat {
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, status, is_my_contact, attribution_asked, staff_user_id) \
         VALUES ($1, $2, 'active', false, false, $3) ON CONFLICT (chat_id) DO UPDATE SET staff_user_id = $3",
    )
    .bind(Uuid::new_v4())
    .bind(cid)
    .bind(staff)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(cid)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// A pending card written directly (what executor.propose stores at level 1).
async fn card(
    db: &mut PgConnection,
    action: &str,
    payload: Value,
    diff: Value,
    price: Value,
    rationale: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, level, customer_facing, payload, rationale, diff, price, status) \
         VALUES ($1, $2, 'nary', 1, true, $3, $4, $5, $6, 'pending')",
    )
    .bind(id)
    .bind(action)
    .bind(&payload)
    .bind(rationale)
    .bind(&diff)
    .bind(&price)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

const LONG: &str = "Good afternoon Mr Smith, this is Nary from Acme Ltd. Last month you asked about \
laptops and monitors for your clinic in Osaka. We wanted to ask whether our laptop quotation has been discussed \
with your team, and whether there is anything we can answer before you decide on the next step? \
If you like, we can schedule a short Google Meet this week.";

#[test]
fn test_c6_a_reengagement_card_shows_the_whole_message() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let cid = c::chat_id();
        sqlx::query("INSERT INTO wa_chats (id, chat_id, display_name, phone, status, is_my_contact, attribution_asked) \
                     VALUES ($1, $2, 'Mr Smith', '+81811', 'active', false, false)")
            .bind(Uuid::new_v4())
            .bind(&cid)
            .execute(&mut *db)
            .await
            .unwrap();
        let cut: String = LONG.chars().take(120).collect();
        let payload = json!({"chat_id": cid, "text": LONG});
        let diff =
            json!({"label": format!("WhatsApp to Mr Smith: {cut}"), "after": {"text": LONG}});
        let text = cards::render(
            &mut db,
            CardView {
                action: "reengagement.send",
                payload: &payload,
                diff: &diff,
                price: &Value::Null,
                rationale: Some("Old chat, no reply for 30 days"),
            },
        )
        .await
        .unwrap();
        assert!(text.contains(LONG), "{text}");
        assert!(text.contains("WhatsApp to Mr Smith (+81811)"), "{text}");
        assert!(
            text.contains("Why: Old chat, no reply for 30 days"),
            "{text}"
        );
        assert!(
            !text.contains(&format!("{cut}\n")),
            "the cut label is left out: {text}"
        );
        assert!(!text.contains('\u{2014}'));
    });
}

#[test]
fn test_c5_a_price_answer_card_shows_the_reply_and_the_prices() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let payload = json!({"channel": "email", "to": "franklin@example.com", "subject": "Re: Laptop prices",
                             "text": "Dear Franklin, the Business Laptop 14 is JPY 125,000 per unit."});
        let price = json!({"currency": "JPY", "quoted": [{"sku": "LAPTOP-14", "price": "125000", "currency": "JPY"}]});
        let text = cards::render(
            &mut db,
            CardView {
                action: "price.answer.send",
                payload: &payload,
                diff: &Value::Null,
                price: &price,
                rationale: Some("Price answer (list price, asked by the customer)"),
            },
        )
        .await
        .unwrap();
        assert!(text.contains("Email to franklin@example.com"), "{text}");
        assert!(text.contains("Subject: Re: Laptop prices"), "{text}");
        assert!(text.contains("is JPY 125,000 per unit."), "{text}");
        assert!(
            text.contains("• LAPTOP-14: JPY 125,000 (list price)"),
            "{text}"
        );
    });
}

#[test]
fn test_c7_a_quotation_card_lists_every_line_and_the_total() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let payload = json!({"subject": "2 x Business Laptop 14", "currency": "JPY", "payment_terms": "50% on order",
                             "items": [{"product_id": Uuid::new_v4().to_string(), "quantity": "2"}]});
        let price = json!({"currency": "JPY", "total": "330000000", "tax_amount": "0", "lines": [
            {"name": "ACME Business Laptop 14", "quantity": "2", "unit_price": "150000000", "amount": "300000000",
             "sale_model": "one_time", "discount_pct": "0", "is_optional": false},
            {"name": "ACME Support Plan", "quantity": "1", "unit_price": "30000000", "amount": "30000000",
             "sale_model": "recurring", "billing_interval_months": 12, "term_months": 12, "is_optional": true}]});
        let diff = json!({"label": "2 x Business Laptop 14"});
        let text = cards::render(
            &mut db,
            CardView {
                action: "quotation.create_draft",
                payload: &payload,
                diff: &diff,
                price: &price,
                rationale: None,
            },
        )
        .await
        .unwrap();
        assert!(
            text.contains("• 2 × ACME Business Laptop 14: JPY 150,000,000 each, one-time (perpetual) = JPY 300,000,000"),
            "{text}"
        );
        assert!(
            text.contains("ACME Support Plan: JPY 30,000,000 each, subscription, billed yearly, 12-month term, optional"),
            "{text}"
        );
        assert!(text.contains("Total: JPY 330,000,000"), "{text}");
        assert!(text.contains("Payment terms: 50% on order"), "{text}");
    });
}

#[test]
fn test_c7_the_quotation_draft_keeps_its_priced_lines_for_the_card() {
    use granary_services::pricing::PricedResult;
    let res = PricedResult {
        currency: "USD".into(),
        lines: vec![],
        subtotal: Default::default(),
        discount_amount: Default::default(),
        tax_lines: vec![],
        tax_amount: Default::default(),
        total: Default::default(),
        all_list: true,
        default_tax: None,
        optional: Default::default(),
    };
    let p = granary_services::nary::quotes::card_price(&res);
    assert_eq!(p["currency"], "USD");
    assert!(p["lines"].as_array().unwrap().is_empty());
    assert_eq!(p["total"], "0");
}

#[test]
fn test_c8_a_merge_card_tells_the_two_copies_apart() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let org = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, 'Northwind Traders', false, '{}')")
            .bind(org)
            .execute(&mut *db)
            .await
            .unwrap();
        let (keep, gone) = (Uuid::new_v4(), Uuid::new_v4());
        for (id, email, phone, org_id, days) in [
            (
                keep,
                Some("robert@northwind.example"),
                "+81811111",
                Some(org),
                40,
            ),
            (gone, None, "+81822222", None, 2),
        ] {
            sqlx::query(
                "INSERT INTO contacts (id, first_name, last_name, email, phone, organization_id, is_deleted, custom, created_at) \
                 VALUES ($1, 'Robert', 'Smith', $2, $3, $4, false, '{}', $5)",
            )
            .bind(id)
            .bind(email)
            .bind(phone)
            .bind(org_id)
            .bind(Utc::now() - TimeDelta::days(days))
            .execute(&mut *db)
            .await
            .unwrap();
        }
        let payload = json!({"keep_id": keep.to_string(), "merge_id": gone.to_string()});
        let diff = json!({"label": "Merge Robert Smith into Robert Smith"});
        let text = cards::render(
            &mut db,
            CardView {
                action: "contact.merge",
                payload: &payload,
                diff: &diff,
                price: &Value::Null,
                rationale: None,
            },
        )
        .await
        .unwrap();
        assert!(text.contains("Keep: Robert Smith: email robert@northwind.example *; phone +81811111 *; organization Northwind Traders *"), "{text}");
        assert!(
            text.contains(
                "Merge away: Robert Smith: email none *; phone +81822222 *; organization none *"
            ),
            "{text}"
        );
        assert!(
            text.contains("(* marks what differs between the two)"),
            "{text}"
        );
    });
}

#[test]
fn test_c6_two_view_prints_the_whole_card() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let (user, cid) = c::owner_with_chat(&mut db).await;
        let chat = wa_chat_row(&mut db, &cid, Some(user.id)).await;
        let id = card(
            &mut db,
            "reengagement.send",
            json!({"chat_id": c::chat_id(), "text": LONG}),
            json!({"label": "WhatsApp to someone: Good afternoon Mr"}),
            Value::Null,
            "Old chat",
        )
        .await;
        commands::set_prompt(&mut db, &json!({"kind": "card", "card_id": id.to_string()}))
            .await
            .unwrap();
        let out = commands::handle(&mut db, &chat, "2", None)
            .await
            .unwrap()
            .unwrap();
        assert!(out.contains(LONG), "{out}");
        assert!(out.contains(&format!("/nary/approvals?card={id}")), "{out}");
        commands::set_prompt(&mut db, &json!({})).await.unwrap();
    });
}

#[test]
fn test_c4_a_topic_notice_after_the_card_does_not_turn_the_1_into_a_question() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        c::working_hours(true);
        let (user, cid) = c::owner_with_chat(&mut db).await;
        let chat = wa_chat_row(&mut db, &cid, Some(user.id)).await;
        let (levels,): (String,) =
            sqlx::query_as("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
                .fetch_one(&mut *db)
                .await
                .unwrap();
        let mut table: Value = serde_json::from_str(&levels).unwrap();
        table["reminder.add"] = json!(1);
        let old = c::set_settings(&mut db, &[("nary.autonomy.levels", &table.to_string())]).await;
        let actor = granary_services::nary::roles::nary_actor(&mut db)
            .await
            .unwrap();
        let row = Box::pin(executor::propose(
            &mut db,
            "reminder.add",
            json!({"entity_type": "whatsapp_chat", "chat_id": cid, "title": "Call Mr Smith back"}),
            actor.as_ref(),
            executor::ProposeOpts {
                rationale: Some("He asked for a call"),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
        c::restore(&mut db, old).await;
        commands::set_prompt(
            &mut db,
            &json!({"kind": "card", "card_id": row.id.to_string()}),
        )
        .await
        .unwrap();
        // an alert lands in the same conversation a few seconds later (a topic notice, not an answer)
        let conv = granary_services::nary::staff_chat::conversation_for(&mut db, &user, false)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls, created_at) \
                     VALUES ($1, $2, 'assistant', '*Meanwhile*', $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(conv.id)
        .bind(json!({"topics": [Uuid::new_v4().to_string()]}))
        .bind(Utc::now() + TimeDelta::seconds(10))
        .execute(&mut *db)
        .await
        .unwrap();
        let prompt = commands::get_prompt(&mut db).await.unwrap().unwrap();
        assert!(!commands::is_stale(&mut db, &chat, &prompt).await.unwrap());
        let out = Box::pin(commands::handle(&mut db, &chat, "1", None))
            .await
            .unwrap();
        assert_eq!(out.as_deref(), Some("Applied 1."));
        let (status,): (String,) =
            sqlx::query_as("SELECT status FROM agent_pending_actions WHERE id = $1")
                .bind(row.id)
                .fetch_one(&mut *db)
                .await
                .unwrap();
        assert_eq!(status, "applied");
    });
}

#[test]
fn test_c4_an_apply_with_edits_the_owner_never_named_is_applied_as_shown() {
    use granary_services::ai::tools::nary_owner;
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let (user, _cid) = c::owner_with_chat(&mut db).await;
        let id = card(
            &mut db,
            "reengagement.send",
            json!({"chat_id": c::chat_id(), "text": LONG}),
            json!({"label": "WhatsApp"}),
            Value::Null,
            "Old chat",
        )
        .await;
        let conv = granary_services::nary::staff_chat::conversation_for(&mut db, &user, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content) VALUES ($1, $2, 'user', '1')")
            .bind(Uuid::new_v4())
            .bind(conv.id)
            .execute(&mut *db)
            .await
            .unwrap();
        let out = nary_owner::_decide_card(
            &mut db,
            &user,
            json!({"card_id": id.to_string(), "decision": "apply", "edits": {"text": "A rewrite"}}),
        )
        .await
        .unwrap();
        assert_eq!(out["proposal"]["edited_payload"], Value::Null, "{out}");
        assert!(
            out["note"].as_str().unwrap().contains("exactly as shown"),
            "{out}"
        );
        // the Owner named a change: the edit is kept
        sqlx::query(
            "INSERT INTO ai_messages (id, conversation_id, role, content, created_at) \
                     VALUES ($1, $2, 'user', 'send it but change it to Mr Smith', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(conv.id)
        .bind(Utc::now() + TimeDelta::seconds(5))
        .execute(&mut *db)
        .await
        .unwrap();
        let out = nary_owner::_decide_card(
            &mut db,
            &user,
            json!({"card_id": id.to_string(), "decision": "apply", "edits": {"text": "Mr Smith"}}),
        )
        .await
        .unwrap();
        assert_eq!(out["proposal"]["edited_payload"]["text"], "Mr Smith");
    });
}

#[test]
fn test_c9_an_urgent_quotation_request_tops_the_brief_worded_as_what_they_want() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        for i in 0..3 {
            card(
                &mut db,
                "crm.record_change",
                json!({"object_type": "lead", "action": "update", "fields": {"priority": "low"}}),
                json!({"label": format!("Lead {i}: priority low")}),
                Value::Null,
                "Routine tidy-up",
            )
            .await;
        }
        let franklin = card(
            &mut db,
            "crm.record_change",
            json!({"object_type": "lead", "action": "create",
                   "fields": {"title": "Franklin (Globex Corp)", "description": "Please send a quotation for 4 laptops, it is urgent"}}),
            json!({"label": "Create lead Franklin (Globex Corp)"}),
            Value::Null,
            "New enquiry on WhatsApp",
        )
        .await;
        let data = briefs::collect(&mut db).await.unwrap();
        let first = &data["cards"][0];
        assert_eq!(first["id"], franklin.to_string(), "{data}");
        assert_eq!(
            first["action"],
            "Franklin (Globex Corp) wants a quotation, urgently"
        );
        let plain = briefs::render_plain(&data, "morning", None);
        assert!(
            plain.contains("1. Franklin (Globex Corp) wants a quotation, urgently: "),
            "{plain}"
        );
        assert!(plain.find("Routine tidy-up").unwrap() > plain.find("Franklin").unwrap());
        // a draft of ours that mentions a quotation does not count as the customer asking
        assert!(!briefs::wants("").quote);
        assert!(briefs::wants("Please send the price list right away").urgent);
        assert!(briefs::wants("Please send the quotation soon").quote);
    });
}

#[test]
fn test_c10_catch_up_sends_the_newest_brief_and_one_line_per_repeated_alert() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let ago = |h: i64| Utc::now() - TimeDelta::hours(h);
        let mut ids = vec![];
        for (kind, tag, slot, at) in [
            ("brief", "Morning brief", "morning", ago(50)),
            ("brief", "Wrap-up", "wrapup", ago(48)),
            ("brief", "Morning brief", "morning", ago(1)),
            ("alert", "WhatsApp or Nary needs attention", "", ago(3)),
            ("alert", "WhatsApp or Nary needs attention", "", ago(2)),
            ("alert", "WhatsApp or Nary needs attention", "", ago(1)),
        ] {
            let id = Uuid::new_v4();
            let facts = if slot.is_empty() {
                Value::Null
            } else {
                json!({"slot": slot})
            };
            sqlx::query(
                "INSERT INTO nary_owner_topics (id, kind, tag, text, facts, status, urgent, anytime, send_after, created_at) \
                 VALUES ($1, $2, $3, $4, $5, 'queued', false, true, $6, $6)",
            )
            .bind(id)
            .bind(kind)
            .bind(tag)
            .bind(format!("Customers may not be acknowledged: WhatsApp is {}.", if kind == "alert" { "disconnected" } else { "fine" }))
            .bind(&facts)
            .bind(at)
            .execute(&mut *db)
            .await
            .unwrap();
            ids.push(id);
        }
        owner_channel::_prune_queue(&mut db).await.unwrap();
        let queued: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, text FROM nary_owner_topics WHERE status = 'queued' ORDER BY created_at",
        )
        .fetch_all(&mut *db)
        .await
        .unwrap();
        let kept: Vec<Uuid> = queued.iter().map(|q| q.0).collect();
        // yesterday's morning brief and wrap-up were overtaken by today's morning brief; three alerts are one
        assert_eq!(kept, vec![ids[2], ids[5]], "{queued:?}");
        assert!(
            queued[1].1.contains("WhatsApp is disconnected")
                && queued[1].1.contains("came up 3 times"),
            "{}",
            queued[1].1
        );
        // pruning again does not add to the count
        owner_channel::_prune_queue(&mut db).await.unwrap();
        let (text,): (String,) = sqlx::query_as("SELECT text FROM nary_owner_topics WHERE id = $1")
            .bind(ids[5])
            .fetch_one(&mut *db)
            .await
            .unwrap();
        assert!(
            text.contains("came up 3 times") && text.matches("came up").count() == 1,
            "{text}"
        );
    });
}

#[test]
fn test_c10_problems_with_the_same_title_are_one_line_with_a_count_and_what_it_is() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let owner = c::owner(&mut db).await;
        for (m, body) in [(50, "Customers may not be acknowledged: WhatsApp is disconnected."),
                          (30, "Customers may not be acknowledged: 2 customer(s) are still owed an acknowledgement."),
                          (10, "Customers may not be acknowledged: WhatsApp is disconnected.")] {
            sqlx::query("INSERT INTO notifications (id, user_id, type, title, body, entity_type, entity_id, created_at) \
                         VALUES ($1, $2, 'nary_error', 'WhatsApp or Nary needs attention', $3, 'nary_error', 'nary.health', $4)")
                .bind(Uuid::new_v4())
                .bind(owner.id)
                .bind(body)
                .bind(Utc::now() - TimeDelta::minutes(m))
                .execute(&mut *db)
                .await
                .unwrap();
        }
        let data = briefs::collect(&mut db).await.unwrap();
        let errors = data["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{data}");
        let plain = briefs::render_plain(&data, "morning", None);
        assert!(
            plain.contains("WhatsApp or Nary needs attention (3 times): Customers may not be acknowledged: WhatsApp is disconnected."),
            "{plain}"
        );
    });
}

#[test]
fn test_c10_several_slots_due_at_once_send_only_the_latest() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let old = c::set_settings(
            &mut db,
            &[(
                "nary.brief_times",
                &json!({"morning": "08:00", "midday": "12:00", "wrapup": "17:00"}).to_string(),
            )],
        )
        .await;
        let due = vec!["morning".to_string(), "midday".to_string()];
        assert_eq!(
            rhythm::latest_slot(&mut db, &due).await.unwrap().as_deref(),
            Some("midday")
        );
        let all = vec![
            "morning".to_string(),
            "wrapup".to_string(),
            "midday".to_string(),
        ];
        assert_eq!(
            rhythm::latest_slot(&mut db, &all).await.unwrap().as_deref(),
            Some("wrapup")
        );
        assert_eq!(rhythm::latest_slot(&mut db, &[]).await.unwrap(), None);
        c::restore(&mut db, old).await;
    });
}

#[test]
fn test_c11_an_overdue_follow_up_is_listed_once_and_can_be_snoozed_done_or_dropped() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        c::working_hours(true);
        let (user, cid) = c::owner_with_chat(&mut db).await;
        let chat = wa_chat_row(&mut db, &cid, Some(user.id)).await;
        let mut rem = vec![];
        for title in [
            "Call Acme Ltd",
            "Send Globex Corp the brochure",
            "Chase Northwind Traders",
        ] {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO crm_reminders (id, owner_user_id, source, entity_type, chat_id, title, due_at, status, snooze_count) \
                 VALUES ($1, $2, 'manual', 'whatsapp_chat', $3, $4, $5, 'open', 0)",
            )
            .bind(id)
            .bind(user.id)
            .bind(&cid)
            .bind(title)
            .bind(Utc::now() - TimeDelta::days(28))
            .execute(&mut *db)
            .await
            .unwrap();
            rem.push(id);
        }
        let brain = Brain::new();
        brain.set(
            "nary.brief",
            Answer::fail(|| AiError::Other("provider down".into())),
        );
        let _seam = brain.install().await;
        let first = briefs::send_brief(&mut db, "morning").await.unwrap();
        let text = first["text"].as_str().unwrap();
        assert!(
            text.contains("Overdue follow-ups (reply *done N*, *snooze N* or *drop N*):"),
            "{text}"
        );
        assert!(text.contains("Call Acme Ltd (still open since "), "{text}");
        // the prompt maps the shown numbers to the follow-ups
        let prompt = commands::get_prompt(&mut db).await.unwrap().unwrap();
        let fu = prompt["followups"].as_object().unwrap().clone();
        assert_eq!(fu.len(), 3, "{prompt}");
        // the next brief does not repeat them, it counts them
        let data = briefs::collect(&mut db).await.unwrap();
        assert!(
            data["overdue_followups"].as_array().unwrap().is_empty(),
            "{data}"
        );
        assert_eq!(data["overdue_seen_total"], 3);
        let plain = briefs::render_plain(&data, "morning", None);
        assert!(
            plain.contains("Overdue follow-ups listed before and still open: 3"),
            "{plain}"
        );
        // done / snooze / drop by number
        let n_of = |id: Uuid| {
            fu.iter()
                .find(|(_, v)| v.as_str() == Some(&id.to_string()))
                .map(|(k, _)| k.clone())
                .unwrap()
        };
        let reply = commands::handle(&mut db, &chat, &format!("done {}", n_of(rem[0])), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, "Marked done: Call Acme Ltd.");
        let reply = commands::handle(&mut db, &chat, &format!("snooze {}", n_of(rem[1])), None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            reply.starts_with("Snoozed 3 days: Send Globex Corp"),
            "{reply}"
        );
        let reply = commands::handle(&mut db, &chat, &format!("drop {}", n_of(rem[2])), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, "Dropped: Chase Northwind Traders.");
        let reply = commands::handle(&mut db, &chat, "done 99", None)
            .await
            .unwrap()
            .unwrap();
        assert!(reply.contains("changed nothing"), "{reply}");
        let rows: Vec<(Uuid, String, chrono::DateTime<Utc>)> =
            sqlx::query_as("SELECT id, status, due_at FROM crm_reminders WHERE id = ANY($1)")
                .bind(&rem)
                .fetch_all(&mut *db)
                .await
                .unwrap();
        let get = |id: Uuid| rows.iter().find(|r| r.0 == id).unwrap().clone();
        assert_eq!(get(rem[0]).1, "done");
        assert_eq!(get(rem[2]).1, "cancelled");
        assert!(get(rem[1]).1 == "open" && get(rem[1]).2 > Utc::now() + TimeDelta::days(2));
        commands::set_prompt(&mut db, &json!({})).await.unwrap();
    });
}

#[test]
fn test_c12_a_write_up_with_its_numbers_out_of_order_falls_back_to_the_plain_list() {
    assert!(briefs::numbers_in_order(
        "Waiting:\n1. a\n2. b\n\nLeads:\n3. c"
    ));
    assert!(!briefs::numbers_in_order("Waiting:\n2. b\n3. c\n1. a"));
    assert!(!briefs::numbers_in_order("*12.* x\n*13.* y\n*11.* z"));
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let data = json!({
            "cards": [{"id": "a", "action": "Send quotation", "what": "Customer 0"},
                      {"id": "b", "action": "Send quotation", "what": "Customer 1"}],
            "new_leads": [], "due_followups": [], "overdue_deals": [], "errors": [], "cards_total": 2,
        });
        let brain = Brain::new();
        brain.set(
            "nary.brief",
            Answer::result(granary_services::ai::base::ChatResult {
                text: "2. Send quotation: Customer 1\n1. Send quotation: Customer 0".into(),
                ..Default::default()
            }),
        );
        let _seam = brain.install().await;
        let (text, by_model) = briefs::compose(&mut db, &data, "morning", None)
            .await
            .unwrap();
        assert!(by_model.is_none());
        assert_eq!(text, briefs::render_plain(&data, "morning", None));
        assert!(briefs::brief_system("morning", "English").contains("never reorder or renumber"));
    });
}

#[test]
fn test_c13_the_more_count_is_the_real_number_waiting() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let live = card(
            &mut db,
            "reminder.add",
            json!({}),
            Value::Null,
            Value::Null,
            "x",
        )
        .await;
        let decided = card(
            &mut db,
            "reminder.add",
            json!({}),
            Value::Null,
            Value::Null,
            "y",
        )
        .await;
        sqlx::query("UPDATE agent_pending_actions SET status = 'applied' WHERE id = $1")
            .bind(decided)
            .execute(&mut *db)
            .await
            .unwrap();
        let mut shown = Uuid::nil();
        for (i, (kind, ref_id)) in [
            ("question", None),
            ("approval", Some(live)),
            ("approval", Some(live)), // the same card raised twice
            ("approval", Some(decided)),
            ("alert", None),
        ]
        .into_iter()
        .enumerate()
        {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO nary_owner_topics (id, kind, tag, text, status, urgent, anytime, ref_type, ref_id, send_after, created_at) \
                 VALUES ($1, $2, $3, 'x', 'queued', false, false, $4, $5, now(), now() + make_interval(secs => $6::int))",
            )
            .bind(id)
            .bind(kind)
            .bind(format!("t{i}"))
            .bind(ref_id.map(|_| "nary_card"))
            .bind(ref_id)
            .bind(i as i32)
            .execute(&mut *db)
            .await
            .unwrap();
            if i == 0 {
                shown = id;
            }
        }
        owner_channel::_prune_queue(&mut db).await.unwrap();
        // after the question: the live card (once) and the alert
        assert_eq!(
            owner_channel::_waiting_after(&mut db, shown).await.unwrap(),
            2
        );
    });
}

#[test]
fn test_c14_inline_variants_of_the_form_sources_are_known() {
    let routes = json!({"inline_whatsapp": "whatsapp", "web3forms": "email", "inline_web3forms": "email", "whatsapp": "whatsapp"});
    assert_eq!(
        web3forms::route_key("inline_fr_web3forms", &routes).as_deref(),
        Some("inline_web3forms")
    );
    assert_eq!(
        web3forms::route_key("inline_en_whatsapp", &routes).as_deref(),
        Some("inline_whatsapp")
    );
    assert_eq!(
        web3forms::route_key("fr_web3forms", &routes).as_deref(),
        Some("web3forms")
    );
    assert_eq!(
        web3forms::route_key("web3forms", &routes).as_deref(),
        Some("web3forms")
    );
    assert_eq!(
        web3forms::route_key("Inline_FR_Web3forms", &routes).as_deref(),
        Some("inline_web3forms")
    );
    assert_eq!(web3forms::route_key("partner_portal", &routes), None);
    let parsed = web3forms::parse_text(Some(
        "name : Robert Smith\nemail : robert@example.com\nsubmission_source : inline_fr_web3forms\n",
    ));
    let known: Vec<String> = ["name", "email", "submission_source"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let chk = web3forms::check(&parsed, None, &known, &[], &routes);
    assert!(
        !chk.problems
            .iter()
            .any(|p| p.contains("unknown submission_source")),
        "{:?}",
        chk.problems
    );
    let chk = web3forms::check(
        &web3forms::parse_text(Some(
            "name : Robert Smith\nsubmission_source : partner_portal\n",
        )),
        None,
        &known,
        &[],
        &routes,
    );
    assert!(chk
        .problems
        .iter()
        .any(|p| p.contains("unknown submission_source 'partner_portal'")));
}
