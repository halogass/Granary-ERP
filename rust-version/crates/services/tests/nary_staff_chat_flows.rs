//! backend/tests/test_nary_staff_chat.py, test_nary_pacing.py, the chat parts of test_nary_chat_fixes.py and
//! test_nary_backlog_oct13.py: Nary's WhatsApp chat with linked staff (services/nary/staff_chat.py and
//! commands.py), with `run_assistant`, the voice-note media and the ERP's own API (`_as_user`) faked as the
//! Python tests monkeypatch them. See nary_chat_common for what is not identical.

mod nary_chat_common;

use chrono::{TimeDelta, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use granary_services::nary::{commands, staff_chat};
use nary_chat_common as c;
use nary_chat_common::{FakeAssistant, FakeErp};

const TAG: &str = "n3_staff_chat";

fn reminder_card(cid: &str, title: &str) -> Value {
    let due =
        granary_compat::pyjson::PyDateTime::from_chrono_utc(&(Utc::now() + TimeDelta::days(2)))
            .isoformat();
    json!({"kind": "followup_reminder", "title": title, "note": null, "entity_type": "whatsapp_chat",
           "entity_id": null, "chat_id": cid, "due_at": due, "summary": format!("Remind you: {title}")})
}

fn last(v: &[String]) -> String {
    v.last().cloned().unwrap_or_default()
}

// ─── test_nary_staff_chat.py ─────────────────────────────────────────────────

#[test]
#[ignore = "covered by crates/api/tests/whatsapp_routes.rs::test_the_webhook_hands_a_staff_message_to_nary_but_not_a_customers (wave 4)"]
fn test_the_webhook_hands_a_staff_message_to_nary_but_not_a_customers() {}

#[test]
fn test_nary_answers_in_whatsapp_style_with_the_persons_own_engine() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let fake = FakeAssistant::new(
            "## Open deals\n\n**3 deals** open:\n- Vandelay Ltd\n- Stark Co\n\n| Deal | Stage |\n|---|---|\n| Vandelay Ltd | Proposal |",
            vec![],
        );
        let (person, cid) = c::staff(&mut db).await;
        let out = staff_chat::seams::scope(
            c::seams_with(Some(fake.clone()), None),
            c::say(&mut db, &cid, "how many open deals do I have?"),
        )
        .await;
        assert_eq!(out["status"], "answered");
        let reply = c::sent_to(&cid).await[0].clone();
        assert!(
            reply.contains("*Open deals*")
                && reply.contains("*3 deals*")
                && reply.contains("• Vandelay Ltd"),
            "{reply}"
        );
        assert!(
            reply.contains("Vandelay Ltd · Proposal")
                && !reply.contains("**")
                && !reply.contains('|')
        );
        let seen = fake.seen();
        assert_eq!(seen[0]["user"], person.id.to_string());
        assert_eq!(seen[0]["context"], json!({"channel": "WhatsApp"}));
        let conv = c::conversation(
            &mut db,
            Uuid::parse_str(seen[0]["conversation"].as_str().unwrap()).unwrap(),
        )
        .await;
        assert_eq!(conv.title.as_deref(), Some("WhatsApp"));
        assert_eq!(conv.user_id, person.id); // in their Nary → Chat history
    });
}

#[test]
fn test_one_ongoing_conversation_a_new_one_after_quiet_hours_or_on_request() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let fake = FakeAssistant::new("OK", vec![]);
        let (_person, cid) = c::staff(&mut db).await;
        staff_chat::seams::scope(c::seams_with(Some(fake.clone()), None), async {
            c::say(&mut db, &cid, "first question").await;
            c::say(&mut db, &cid, "second question").await;
            let seen = fake.seen();
            assert_eq!(seen[0]["conversation"], seen[1]["conversation"]);
            c::say(&mut db, &cid, "Nary new").await;
            assert_eq!(last(&c::sent_to(&cid).await), "New conversation started.");
            c::say(&mut db, &cid, "third question").await;
            let seen = fake.seen();
            assert_ne!(seen[2]["conversation"], seen[1]["conversation"]);
            // 13 quiet hours later
            sqlx::query("UPDATE ai_conversations SET updated_at = $2 WHERE id = $1")
                .bind(Uuid::parse_str(seen[2]["conversation"].as_str().unwrap()).unwrap())
                .bind(Utc::now() - TimeDelta::hours(13))
                .execute(&mut *db)
                .await
                .unwrap();
            c::say(&mut db, &cid, "empat").await;
            let seen = fake.seen();
            assert_ne!(seen[3]["conversation"], seen[2]["conversation"]);
        })
        .await;
    });
}

#[test]
fn test_cards_come_back_numbered_and_a_number_applies_them_as_that_person() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (person, cid) = c::staff(&mut db).await;
        let due =
            granary_compat::pyjson::PyDateTime::from_chrono_utc(&(Utc::now() + TimeDelta::days(2)))
                .isoformat();
        let title = c::u("Call Vandelay Ltd");
        let card = json!({"kind": "followup_reminder", "title": title, "note": null, "entity_type": "whatsapp_chat",
                          "entity_id": null, "chat_id": cid, "due_at": due, "summary": "Remind you to call Vandelay Ltd on Tue"});
        let mut other = card.clone();
        other["title"] = json!(c::u("Not this one"));
        other["summary"] = json!("Remind you about something else");
        let fake = FakeAssistant::new("I prepared two reminders.", vec![card, other.clone()]);
        let erp = FakeErp::new();
        staff_chat::seams::scope(c::seams_with(Some(fake), Some(erp)), async {
            c::say(&mut db, &cid, "remind me to call Vandelay Ltd the day after tomorrow").await;
            let listing = last(&c::sent_to(&cid).await);
            assert!(listing.contains("1. Remind you to call Vandelay Ltd") && listing.contains("2. Remind you about something else"));
            assert!(listing.contains("Reply with the numbers"));
            let out = c::say(&mut db, &cid, "1").await;
            assert_eq!(out["status"], "answered_cards");
            assert!(last(&c::sent_to(&cid).await).starts_with("1. Done: Remind you to call Vandelay Ltd"));
            let rows = c::reminders(&mut db, &title).await;
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, person.id);
            assert!(c::reminders(&mut db, other["title"].as_str().unwrap()).await.is_empty());
            let tc: (Value,) = sqlx::query_as(
                "SELECT tool_calls FROM ai_messages WHERE role = 'assistant' AND content = 'I prepared two reminders.' \
                 AND tool_calls::text LIKE $1 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(format!("%{title}%"))
            .fetch_one(&mut *db)
            .await
            .unwrap();
            let st: Vec<&str> = tc.0["proposals"].as_array().unwrap().iter().map(|p| p["status"].as_str().unwrap()).collect();
            assert_eq!(st, ["applied", "discarded"]);
            c::say(&mut db, &cid, "1").await; // nothing waiting any more
            assert_ne!(last(&c::sent_to(&cid).await), "1. Done: Remind you to call Vandelay Ltd");
        })
        .await;
    });
}

#[test]
fn test_zero_skips_and_a_failed_card_says_why() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (_person, cid) = c::staff(&mut db).await;
        let past_due =
            granary_compat::pyjson::PyDateTime::from_chrono_utc(&(Utc::now() - TimeDelta::days(2)))
                .isoformat();
        let past = json!({"kind": "followup_reminder", "title": "Too late", "entity_type": "whatsapp_chat", "chat_id": cid,
                          "due_at": past_due, "summary": "A reminder in the past"});
        let erp = FakeErp::new();
        staff_chat::seams::scope(
            c::seams_with(
                Some(FakeAssistant::new("Here.", vec![past.clone()])),
                Some(erp.clone()),
            ),
            async {
                c::say(&mut db, &cid, "x").await;
                c::say(&mut db, &cid, "1").await;
            },
        )
        .await;
        assert!(last(&c::sent_to(&cid).await)
            .contains("Not done: A reminder in the past. Pick a time in the future"));
        staff_chat::seams::scope(
            c::seams_with(Some(FakeAssistant::new("Again.", vec![past])), Some(erp)),
            async {
                c::say(&mut db, &cid, "y").await;
                c::say(&mut db, &cid, "0").await;
            },
        )
        .await;
        assert_eq!(last(&c::sent_to(&cid).await), "Skipped. Nothing was saved.");
    });
}

#[test]
fn test_a_voice_note_is_transcribed_and_answered() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let fake = FakeAssistant::new("You have 3 deals.", vec![]);
        let (_person, cid) = c::staff(&mut db).await;
        staff_chat::seams::scope(c::seams_with(Some(fake.clone()), None), async {
            let id = c::ingest(&mut db, &cid, "", None, "ptt").await;
            staff_chat::reply(&mut db, &cid, &id.to_string(), false)
                .await
                .unwrap();
            assert_eq!(fake.seen()[0]["message"], "how many open deals do I have");
            assert!(
                c::sent_to(&cid).await[0].starts_with("_Heard: “how many open deals do I have”_")
            );
            let old = c::set_settings(&mut db, &[("nary.staff_chat.voice", "false")]).await;
            let id = c::ingest(&mut db, &cid, "", None, "ptt").await;
            let out = staff_chat::reply(&mut db, &cid, &id.to_string(), false).await;
            c::restore(&mut db, old).await;
            assert_eq!(out.unwrap()["status"], "voice_off");
            assert!(last(&c::sent_to(&cid).await).starts_with("Please send it as text"));
        })
        .await;
    });
}

#[test]
fn test_a_long_answer_is_split_and_ends_with_a_link_to_the_erp() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let long: Vec<String> = (0..6)
            .map(|i| format!("Paragraph {i}: {}", "x".repeat(900)))
            .collect();
        let fake = FakeAssistant::new(&long.join("\n\n"), vec![]);
        let (_person, cid) = c::staff(&mut db).await;
        let old = c::set_settings(
            &mut db,
            &[
                ("nary.staff_chat.max_bubbles", "3"),
                ("nary.staff_chat.bubble_chars", "1500"),
            ],
        )
        .await;
        staff_chat::seams::scope(
            c::seams_with(Some(fake.clone()), None),
            c::say(&mut db, &cid, "everything"),
        )
        .await;
        c::restore(&mut db, old).await;
        let parts = c::sent_to(&cid).await;
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.chars().count() <= 1500));
        let conv = fake.seen()[0]["conversation"].as_str().unwrap().to_string();
        assert!(last(&parts).contains(&format!("/nary/chat?c={conv}")));
    });
}

#[test]
fn test_the_ceos_number_answers_whichever_nary_sent_last() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let user = c::owner(&mut db).await;
        let cid = c::chat_id();
        c::link_staff_chat(&mut db, user.id, &cid, "Owner phone").await;
        // an approval notice first
        commands::set_prompt(
            &mut db,
            &json!({"kind": "card", "card_id": Uuid::new_v4().to_string()}),
        )
        .await
        .unwrap();
        let fake = FakeAssistant::new(
            "Draft ready.",
            vec![json!({"kind": "sop", "title": "Returns", "payload": {"title": "Returns"}})],
        );
        // then a chat card: newer
        staff_chat::seams::scope(
            c::seams_with(Some(fake), None),
            c::say(&mut db, &cid, "draft an SOP for returns"),
        )
        .await;
        assert!(staff_chat::claims_numbers(&mut db, &cid, Some("1"), None)
            .await
            .unwrap());
        // a newer notice
        commands::set_prompt(
            &mut db,
            &json!({"kind": "card", "card_id": Uuid::new_v4().to_string()}),
        )
        .await
        .unwrap();
        assert!(!staff_chat::claims_numbers(&mut db, &cid, Some("1"), None)
            .await
            .unwrap());
        assert!(
            !staff_chat::claims_numbers(&mut db, &cid, Some("what about deals?"), None)
                .await
                .unwrap()
        );
        commands::set_prompt(&mut db, &json!({})).await.unwrap();
    });
}

#[test]
fn test_switched_off_nary_stays_silent() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let fake = FakeAssistant::new("x", vec![]);
        let (_person, cid) = c::staff(&mut db).await;
        let old = c::set_settings(&mut db, &[("nary.staff_chat.enabled", "false")]).await;
        let out = staff_chat::seams::scope(
            c::seams_with(Some(fake.clone()), None),
            c::say(&mut db, &cid, "hello"),
        )
        .await;
        c::restore(&mut db, old).await;
        assert_eq!(out["status"], "off");
        assert!(fake.seen().is_empty() && c::sent_to(&cid).await.is_empty());
    });
}

// ─── test_nary_pacing.py ─────────────────────────────────────────────────────

#[test]
fn test_an_answer_is_cut_into_parts_at_the_marker() {
    assert_eq!(
        staff_chat::split_parts("One.\n[[next]]\nTwo.\n\n[[NEXT]] Three."),
        ["One.", "Two.", "Three."]
    );
    assert_eq!(staff_chat::split_parts("Just one."), ["Just one."]);
}

#[test]
fn test_the_first_part_goes_now_and_the_rest_wait_for_next() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (_p, cid) = c::staff(&mut db).await;
        let fake = FakeAssistant::new(
            "Globex wants 300 laptops.\n[[next]]\nThe draft reply.\n[[next]]\nNorthwind is waiting too.",
            vec![],
        );
        staff_chat::seams::scope(c::seams_with(Some(fake.clone()), None), async {
            c::say(&mut db, &cid, "What's new?").await;
            let first = last(&c::sent_to(&cid).await);
            assert!(
                first.starts_with("Globex wants 300 laptops") && !first.contains("The draft reply")
            );
            assert!(first.contains('2') && (first.contains("next")));
            c::say(&mut db, &cid, "next").await;
            assert!(last(&c::sent_to(&cid).await).starts_with("The draft reply"));
            c::say(&mut db, &cid, "continue").await;
            assert_eq!(last(&c::sent_to(&cid).await), "Northwind is waiting too.");
            c::say(&mut db, &cid, "next").await;
            let l = last(&c::sent_to(&cid).await);
            assert!(l == staff_chat::QUEUE_PHRASES_EN.none, "{l}");
            assert_eq!(fake.seen().len(), 1); // "next" never calls the model
        })
        .await;
    });
}

#[test]
fn test_enough_drops_what_is_waiting() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (_p, cid) = c::staff(&mut db).await;
        staff_chat::seams::scope(
            c::seams_with(
                Some(FakeAssistant::new("Part one.\n[[next]]\nPart two.", vec![])),
                None,
            ),
            async {
                c::say(&mut db, &cid, "Hi").await;
                c::say(&mut db, &cid, "enough").await;
                c::say(&mut db, &cid, "next").await;
            },
        )
        .await;
        assert!(!c::sent_to(&cid).await.iter().any(|m| m == "Part two."));
    });
}

#[test]
fn test_the_pacing_rule_is_given_to_the_model_on_whatsapp() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (_p, cid) = c::staff(&mut db).await;
        let fake = std::sync::Arc::new(FakeAssistant {
            text: "ok".into(),
            proposals: vec![],
            store: false,
            fail: None,
            seen: Default::default(),
        });
        let old = c::set_settings(&mut db, &[("nary.staff_chat.max_lines", "6")]).await;
        staff_chat::seams::scope(
            c::seams_with(Some(fake.clone()), None),
            c::say(&mut db, &cid, "Hi"),
        )
        .await;
        c::restore(&mut db, old).await;
        let extra = fake.seen()[0]["extra"].as_str().unwrap().to_string();
        assert!(extra.contains("ONE TOPIC AT A TIME") && extra.contains("at most 6 short lines"));
    });
}

// ─── test_nary_chat_fixes.py (the chat parts) ────────────────────────────────

async fn shown(db: &mut sqlx::PgConnection, user_id: Uuid, ago: TimeDelta, index: usize) {
    let conv = staff_chat::_latest(db, user_id).await.unwrap().unwrap();
    let rows: Vec<(Uuid, Option<Value>)> = sqlx::query_as(
        "SELECT id, tool_calls FROM ai_messages WHERE conversation_id = $1 AND role = 'assistant' ORDER BY created_at DESC",
    )
    .bind(conv.id)
    .fetch_all(&mut *db)
    .await
    .unwrap();
    let (id, tc) = rows[index].clone();
    let mut m = tc.and_then(|v| v.as_object().cloned()).unwrap_or_default();
    m.insert(
        "shown_at".into(),
        json!(granary_compat::pyjson::PyDateTime::from_chrono_utc(&(Utc::now() - ago)).isoformat()),
    );
    sqlx::query("UPDATE ai_messages SET tool_calls = $2 WHERE id = $1")
        .bind(id)
        .bind(Value::Object(m))
        .execute(&mut *db)
        .await
        .unwrap();
}

#[test]
fn test_a_number_sent_before_the_cards_arrived_applies_nothing() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (_person, cid) = c::staff(&mut db).await;
        let title = c::u("Call Early Ltd");
        staff_chat::seams::scope(
            c::seams_with(
                Some(FakeAssistant::new(
                    "Prepared.",
                    vec![reminder_card(&cid, &title)],
                )),
                Some(FakeErp::new()),
            ),
            async {
                c::say(&mut db, &cid, "remind me").await;
                let out = c::say_at(
                    &mut db,
                    &cid,
                    "1",
                    Some(Utc::now() - TimeDelta::seconds(20)),
                    false,
                )
                .await;
                assert_eq!(out["status"], "answered_cards");
                assert!(last(&c::sent_to(&cid).await)
                    .starts_with("Your reply reached me before those cards did"));
                assert!(c::reminders(&mut db, &title).await.is_empty());
            },
        )
        .await;
    });
}

#[test]
fn test_numbers_that_dont_fit_the_list_apply_nothing() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (_person, cid) = c::staff(&mut db).await;
        let title = c::u("Only card");
        staff_chat::seams::scope(
            c::seams_with(
                Some(FakeAssistant::new(
                    "Prepared.",
                    vec![reminder_card(&cid, &title)],
                )),
                Some(FakeErp::new()),
            ),
            async {
                c::say(&mut db, &cid, "remind me").await;
                c::say(&mut db, &cid, "1 2 3 4").await;
                assert!(last(&c::sent_to(&cid).await)
                    .starts_with("Those numbers don't match: 1 card is waiting"));
                assert!(c::reminders(&mut db, &title).await.is_empty());
                c::say(&mut db, &cid, "1").await; // the card is still there
                assert!(last(&c::sent_to(&cid).await).starts_with("1. Done: Remind you"));
                assert_eq!(c::reminders(&mut db, &title).await.len(), 1);
            },
        )
        .await;
    });
}

/// 12 Oct 15:43: four Globex cards on the phone, "1 2 3 4" typed, an Adam card lands, the numbers took it.
#[test]
fn test_a_number_answers_the_list_on_screen_not_one_that_arrived_after() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (person, cid) = c::staff(&mut db).await;
        let (a1, a2, b) = (
            c::u("Globex email"),
            c::u("Globex WhatsApp"),
            c::u("Adam WhatsApp"),
        );
        let erp = FakeErp::new();
        staff_chat::seams::scope(
            c::seams_with(
                Some(FakeAssistant::new(
                    "Four cards.",
                    vec![reminder_card(&cid, &a1), reminder_card(&cid, &a2)],
                )),
                Some(erp.clone()),
            ),
            c::say(&mut db, &cid, "ask him more"),
        )
        .await;
        shown(&mut db, person.id, TimeDelta::seconds(60), 0).await;
        staff_chat::seams::scope(
            c::seams_with(
                Some(FakeAssistant::new(
                    "One card.",
                    vec![reminder_card(&cid, &b)],
                )),
                Some(erp.clone()),
            ),
            async {
                c::say(&mut db, &cid, "and the other one").await; // its card reaches the phone now
                c::say_at(
                    &mut db,
                    &cid,
                    "1 2",
                    Some(Utc::now() - TimeDelta::seconds(30)),
                    false,
                )
                .await;
            },
        )
        .await;
        assert_eq!(c::reminders(&mut db, &a1).await.len(), 1);
        assert_eq!(c::reminders(&mut db, &a2).await.len(), 1);
        assert!(c::reminders(&mut db, &b).await.is_empty());
        let l = last(&c::sent_to(&cid).await);
        assert!(
            l.contains("the newer ones below are still waiting") && l.contains(&b),
            "{l}"
        );
    });
}

#[test]
fn test_a_owner_command_number_sent_before_the_notice_applies_nothing() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        commands::set_prompt(
            &mut db,
            &json!({"kind": "card", "card_id": Uuid::new_v4().to_string()}),
        )
        .await
        .unwrap();
        let at =
            commands::prompt_time(commands::get_prompt(&mut db).await.unwrap().as_ref()).unwrap();
        let chat = c::chat_value("x@c.us", None);
        let out = commands::handle(&mut db, &chat, "1", Some(at - TimeDelta::seconds(5)))
            .await
            .unwrap()
            .unwrap();
        assert!(out.starts_with("Your reply was sent before my last notice"));
        commands::set_prompt(&mut db, &json!({"kind": "list", "items": {"1": "a"}}))
            .await
            .unwrap();
        let out = commands::handle(&mut db, &chat, "1 2", None)
            .await
            .unwrap()
            .unwrap();
        assert!(out.starts_with("Those numbers don't match"));
        assert_eq!(
            out,
            "Those numbers don't match the list I sent (1), so I applied nothing."
        );
        commands::set_prompt(&mut db, &json!({})).await.unwrap();
    });
}

#[test]
fn test_one_turn_at_a_time_a_second_message_waits() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let fake = FakeAssistant::new("OK", vec![]);
        let (person, cid) = c::staff(&mut db).await;
        let id = c::ingest(&mut db, &cid, "kedua", None, "chat").await;
        let mut db2 = pool.acquire().await.unwrap();
        let done = staff_chat::seams::scope(c::seams_with(Some(fake.clone()), None), async {
            granary_services::nary::turns::turn(&person.id, async {
                let busy = staff_chat::reply_in_turn(&mut db2, &cid, &id.to_string(), false, 0)
                    .await
                    .unwrap();
                assert_eq!(busy["status"], "busy");
                assert!(fake.seen().is_empty());
                let old = c::set_settings(&mut db2, &[("nary.staff_chat.wait_seconds", "5")]).await;
                let done = staff_chat::reply_in_turn(&mut db2, &cid, &id.to_string(), false, 6)
                    .await
                    .unwrap();
                c::restore(&mut db2, old).await;
                done
            })
            .await
            .unwrap()
        })
        .await;
        assert_eq!(done["status"], "answered"); // waited long enough: answered anyway
        assert_eq!(fake.seen().len(), 1);
        drop(db);
    });
}

#[test]
fn test_a_failed_answer_is_said_in_the_chat() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let broken = std::sync::Arc::new(FakeAssistant {
            text: String::new(),
            proposals: vec![],
            store: true,
            fail: Some("model down".into()),
            seen: Default::default(),
        });
        let (_person, cid) = c::staff(&mut db).await;
        let id = c::ingest(
            &mut db,
            &cid,
            "Can you walk me thru these one by one",
            None,
            "chat",
        )
        .await;
        let res = staff_chat::seams::scope(
            c::seams_with(Some(broken), None),
            staff_chat::reply_in_turn(&mut db, &cid, &id.to_string(), false, 0),
        )
        .await
        .unwrap();
        assert_eq!(res["status"], "failed");
        assert_eq!(res["error"], "RuntimeError");
        let l = last(&c::sent_to(&cid).await);
        assert!(
            l.starts_with("I couldn't answer “Can you walk me thru these one by one” just now"),
            "{l}"
        );
        assert!(l.contains("(RuntimeError)"));
    });
}

#[test]
fn test_a_late_message_is_answered_with_a_note_and_a_late_number_is_not_applied() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (_person, cid) = c::staff(&mut db).await;
        let title = c::u("Late card");
        let old = Some(Utc::now() - TimeDelta::hours(3));
        staff_chat::seams::scope(
            c::seams_with(
                Some(FakeAssistant::new(
                    "Here you go.",
                    vec![reminder_card(&cid, &title)],
                )),
                Some(FakeErp::new()),
            ),
            async {
                let res = c::say_at(&mut db, &cid, "About Globex lead, ask first", old, true).await;
                let sent = c::sent_to(&cid).await;
                assert_eq!(res["status"], "answered");
                assert!(sent[sent.len() - 2].contains("reached me late"));
                let res = c::say_at(&mut db, &cid, "1", old, true).await;
                assert_eq!(res["status"], "late_answer_not_applied");
                assert!(c::reminders(&mut db, &title).await.is_empty());
            },
        )
        .await;
    });
}

#[test]
fn test_the_gap_check_answers_a_missed_staff_message_and_says_the_feed_drops_messages() {
    use granary_services::later::nary_channels::set_nary_staff_chat_reply_delay;
    use granary_services::nary::{channels, wa_ingest};
    use std::sync::{Arc, Mutex};
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let queued: Arc<Mutex<Vec<(String, String, bool)>>> = Arc::new(Mutex::new(vec![]));
        let q = queued.clone();
        set_nary_staff_chat_reply_delay(Some(Arc::new(
            move |chat: &str, msg: &str, late: bool| {
                q.lock()
                    .unwrap()
                    .push((chat.to_string(), msg.to_string(), late));
            },
        )));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_nary_staff_chat_reply_delay(None);
            }
        }
        let _reset = Reset;
        let (_person, cid) = c::staff(&mut db).await;
        // ingest(payload(cid, "first message"))
        let key = Uuid::new_v4().simple().to_string()[..20].to_uppercase();
        let first = json!({"messageId": format!("false_{cid}_{key}"), "chatId": cid, "from": cid, "to": "818000@c.us",
            "body": "first message", "type": "chat", "timestamp": Utc::now().timestamp(), "hasMedia": false,
            "fromMe": false, "ack": 1, "chatName": "Bob Pytest", "isGroup": false, "fromName": "Bob Pytest",
            "isMyContact": false});
        wa_ingest::ingest(&mut db, &first, "live").await.unwrap();
        let ts = (Utc::now() - TimeDelta::minutes(10)).timestamp();
        let fresh = Utc::now().timestamp();
        let h = || Uuid::new_v4().simple().to_string()[..6].to_string();
        let listing = json!([
            {"id": format!("false_{cid}_MISSED{}", h()), "body": "Can you walk me thru these", "fromMe": false,
             "timestamp": ts, "type": "chat"},
            {"id": format!("false_{cid}_FRESH{}", h()), "body": "just now", "fromMe": false,
             "timestamp": fresh, "type": "chat"},
        ]);
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!("/messages/{cid}")))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(listing))
            .mount(c::wa_server())
            .await;
        sqlx::query("DELETE FROM notifications WHERE entity_id = 'wa.webhook_gap'")
            .execute(&mut *db)
            .await
            .unwrap();
        assert_eq!(channels::gap_check(&mut db).await.unwrap(), 1); // the fresh one is left to the webhook
        let queued = queued.lock().unwrap().clone();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].0, cid);
        assert!(queued[0].2); // late=True
        let body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM notifications WHERE entity_id = 'wa.webhook_gap' LIMIT 1",
        )
        .fetch_optional(&mut *db)
        .await
        .unwrap();
        assert!(body.unwrap().contains("never delivered"));
    });
}

#[test]
fn test_a_staff_chat_is_known_by_its_stored_chat_under_another_id() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (person, _) = c::staff(&mut db).await;
        let lid = format!(
            "{:015}@lid",
            Uuid::new_v4().as_u128() % 1_000_000_000_000_000
        );
        sqlx::query("INSERT INTO wa_chats (id, chat_id, phone, status, staff_user_id) VALUES ($1, $2, NULL, 'active', $3)")
            .bind(Uuid::new_v4())
            .bind(&lid)
            .bind(person.id)
            .execute(&mut *db)
            .await
            .unwrap();
        let got = staff_chat::staff_user(&mut db, Some(&lid))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, person.id);
    });
}

#[test]
fn test_a_sent_card_is_named_not_quoted_again() {
    let p = json!({"kind": "customer_message", "channel": "whatsapp", "recipient": "David (+81 816-1955-515)",
                   "text": "Hello ".repeat(50)});
    assert_eq!(
        staff_chat::done_text(&p, "Hello Hello"),
        "Sent: WhatsApp to David (+81 816-1955-515)"
    );
}

#[test]
fn test_a_chat_links_itself_when_the_contact_gets_its_number() {
    // Python goes through crm.writes.apply_change, which calls chat_links.link_by_number (wired at the end
    // of wave 3); here link_by_number is called as apply_change calls it.
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let number = c::phone();
        let lid = format!(
            "{:015}@lid",
            Uuid::new_v4().as_u128() % 1_000_000_000_000_000
        );
        sqlx::query("INSERT INTO wa_chats (id, chat_id, status, display_name) VALUES ($1, $2, 'active', $3)")
            .bind(Uuid::new_v4())
            .bind(&lid)
            .bind(format!("+{number}"))
            .execute(&mut *db)
            .await
            .unwrap();
        let contact = Uuid::new_v4();
        sqlx::query("INSERT INTO contacts (id, first_name, last_name, phone, is_deleted, custom) VALUES ($1, 'Adam', $2, $3, false, '{}')")
            .bind(contact)
            .bind(c::u("Pytest"))
            .bind(format!("+{number}"))
            .execute(&mut *db)
            .await
            .unwrap();
        let n = granary_services::nary::chat_links::link_by_number(&mut db, contact)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let (cid_, phone_): (Option<Uuid>, Option<String>) =
            sqlx::query_as("SELECT contact_id, phone FROM wa_chats WHERE chat_id = $1")
                .bind(&lid)
                .fetch_one(&mut *db)
                .await
                .unwrap();
        assert_eq!(cid_, Some(contact));
        assert_eq!(phone_, Some(format!("+{number}")));
    });
}

#[test]
fn test_a_reminder_card_snoozes_through_its_endpoint() {
    // The card itself comes from ai/tools/catalog (_h_propose_reminder_change, another agent's port); the
    // endpoint it calls is staff_chat._request_for.
    let rid = Uuid::new_v4();
    let card = json!({"kind": "reminder_change", "reminder_id": rid.to_string(), "action": "snooze",
                      "until": "2026-10-18T10:00:00+00:00", "summary": "Move “Mine” to 18 Oct"});
    let (method, path, body) = staff_chat::_request_for(&card).unwrap();
    assert_eq!(
        (method.as_str(), path),
        ("POST", format!("/crm/reminders/{rid}/snooze"))
    );
    assert_eq!(body, Some(json!({"until": card["until"]})));
    let cancel =
        json!({"kind": "reminder_change", "reminder_id": rid.to_string(), "action": "cancel"});
    assert_eq!(staff_chat::_request_for(&cancel).unwrap().2, None);
}

// ─── test_nary_backlog_oct13.py (the chat parts) ─────────────────────────────

async fn owner_with_chat(db: &mut sqlx::PgConnection) -> (granary_core::auth::AuthUser, String) {
    let user = c::owner(db).await;
    let cid = c::chat_id();
    c::link_staff_chat(db, user.id, &cid, "Owner phone").await;
    (user, cid)
}

async fn add_assistant(
    db: &mut sqlx::PgConnection,
    conv: Uuid,
    content: &str,
    tool_calls: Option<Value>,
) {
    sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls) VALUES ($1, $2, 'assistant', $3, $4)")
        .bind(Uuid::new_v4())
        .bind(conv)
        .bind(content)
        .bind(tool_calls)
        .execute(&mut *db)
        .await
        .unwrap();
}

#[test]
fn test_a_remark_without_cards_does_not_hide_the_cards_on_screen() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (user, _) = owner_with_chat(&mut db).await;
        let conv = staff_chat::conversation_for(&mut db, &user, true)
            .await
            .unwrap();
        add_assistant(&mut db, conv.id, "The reply to Alice is ready.",
            Some(json!({"proposals": [{"kind": "nary_pause", "paused": true, "status": "pending", "summary": "Alice reply"}]}))).await;
        add_assistant(&mut db, conv.id, "I've closed the TEST 2 brief.", None).await;
        let (_, waiting) = staff_chat::pending(&mut db, Some(&conv), None)
            .await
            .unwrap();
        assert_eq!(
            waiting
                .iter()
                .map(|p| p["summary"].clone())
                .collect::<Vec<_>>(),
            [json!("Alice reply")]
        );
        // a newer answer with its own cards replaces them
        add_assistant(&mut db, conv.id, "New card.",
            Some(json!({"proposals": [{"kind": "nary_pause", "paused": false, "status": "pending", "summary": "Newer"}]}))).await;
        let (_, waiting) = staff_chat::pending(&mut db, Some(&conv), None)
            .await
            .unwrap();
        assert_eq!(
            waiting
                .iter()
                .map(|p| p["summary"].clone())
                .collect::<Vec<_>>(),
            [json!("Newer")]
        );
    });
}

#[test]
fn test_cards_older_than_the_window_lapse_once_nary_said_something_else() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (user, _) = owner_with_chat(&mut db).await;
        let old = granary_compat::pyjson::PyDateTime::from_chrono_utc(
            &(Utc::now() - TimeDelta::hours(3)),
        )
        .isoformat();
        let conv = staff_chat::conversation_for(&mut db, &user, true)
            .await
            .unwrap();
        add_assistant(&mut db, conv.id, "Card.",
            Some(json!({"shown_at": old, "proposals": [{"kind": "nary_pause", "status": "pending", "summary": "Old"}]}))).await;
        add_assistant(&mut db, conv.id, "Something else.", None).await;
        assert!(staff_chat::pending(&mut db, Some(&conv), None)
            .await
            .unwrap()
            .1
            .is_empty());
    });
}

#[test]
fn test_an_old_numbered_notice_no_longer_captures_a_number() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let (user, cid) = owner_with_chat(&mut db).await;
        commands::set_prompt(
            &mut db,
            &json!({"kind": "card", "card_id": Uuid::new_v4().to_string()}),
        )
        .await
        .unwrap();
        let chat = c::chat_value(&cid, Some(user.id));
        let prompt = commands::get_prompt(&mut db).await.unwrap().unwrap();
        assert!(!commands::is_stale(&mut db, &chat, &prompt).await.unwrap());
        let conv = staff_chat::conversation_for(&mut db, &user, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content, created_at) VALUES ($1, $2, 'assistant', $3, $4)")
            .bind(Uuid::new_v4())
            .bind(conv.id)
            .bind("Diana: 1. add her email, 2. leave it")
            .bind(Utc::now() + TimeDelta::seconds(5))
            .execute(&mut *db)
            .await
            .unwrap();
        assert!(commands::is_stale(&mut db, &chat, &prompt).await.unwrap());
        assert_eq!(
            commands::handle(&mut db, &chat, "2", None).await.unwrap(),
            None
        ); // Nary reads it, not the old notice
        commands::set_prompt(&mut db, &json!({})).await.unwrap();
    });
}

#[test]
fn test_a_brief_with_nothing_waiting_is_not_a_numbered_list() {
    assert_eq!(
        commands::parse(Some("2"), Some(&json!({"kind": "list", "items": {}}))),
        None
    );
    assert_eq!(
        commands::parse(
            Some("2"),
            Some(&json!({"kind": "list", "items": {"1": "a"}}))
        )
        .unwrap()
        .kind,
        "mismatch"
    );
}

#[test]
fn test_the_whatsapp_yes_runs_a_chat_action_through_its_endpoint() {
    let p = json!({"kind": "nary_action", "action": "meeting.cancel", "payload": {"meeting_id": "m1"}, "summary": "Cancel TEST"});
    assert_eq!(
        staff_chat::_request_for(&p).unwrap(),
        (
            "POST".to_string(),
            "/nary/actions/run".to_string(),
            Some(
                json!({"action": "meeting.cancel", "payload": {"meeting_id": "m1"}, "message": null})
            )
        )
    );
    assert_eq!(
        staff_chat::done_text(&p, "Cancelled TEST"),
        "Done: Cancelled TEST"
    );
}

// ─── test_nary_p2_send.py: the Owner's command parsing ─────────────────────────

#[test]
fn test_command_parsing() {
    let card = json!({"kind": "card", "card_id": "c1"});
    let listing = json!({"kind": "list", "items": {"1": "a", "2": "b", "3": "c"}});
    let k = |t: &str, p: Option<&Value>| commands::parse(Some(t), p).map(|c| c.kind);
    assert_eq!(k("Nary pause", None).as_deref(), Some("pause"));
    assert_eq!(k("  nary   RESUME ", None).as_deref(), Some("resume"));
    assert_eq!(k("1", Some(&card)).as_deref(), Some("apply"));
    assert_eq!(k("2", Some(&card)).as_deref(), Some("view"));
    assert_eq!(k("3", Some(&card)).as_deref(), Some("skip"));
    assert_eq!(k("4", Some(&card)), None);
    assert_eq!(
        commands::parse(Some("1 2 3"), Some(&listing))
            .unwrap()
            .numbers,
        [1, 2, 3]
    );
    assert_eq!(
        commands::parse(Some("1,3"), Some(&listing))
            .unwrap()
            .numbers,
        [1, 3]
    );
    assert_eq!(k("2 9", Some(&listing)).as_deref(), Some("mismatch")); // a number not on the list: nothing applies
    assert_eq!(
        k("1", Some(&json!({"kind": "held", "chat_id": "x"}))).as_deref(),
        Some("business")
    );
    assert_eq!(
        k("2", Some(&json!({"kind": "held", "chat_id": "x"}))).as_deref(),
        Some("personal")
    );
    assert_eq!(k("1", None), None); // a bare number with nothing to refer to
    assert_eq!(k("ok thanks 1", Some(&listing)), None); // an ordinary message
}
