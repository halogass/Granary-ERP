//! nary/{cadence, channel_actions} against compat-tests/golden/nary_cadence/*.json, first recorded from the
//! retired Python functions (re-record with GOLDEN_BLESS=1 after an intended change and review the diff):
//! strip_signoff, _html, the nary.cadence.steps checks and the rows start() schedules, the model request a
//! follow-up draft sends (system, content, schema) and its card, and the card labels.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use granary_core::errors::ApiError;
use granary_core::settings_store::{parse_json, require_from};
use granary_services::nary::{cadence, channel_actions, persona};
use serde_json::{json, Value};
use uuid::Uuid;

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/nary_cadence/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn golden_strip_signoff() {
    let cases = load("strip_signoff");
    for c in &cases {
        // The golden inputs sign off as the test company (the configured company_name): a line with
        // just its name is a sign-off too.
        let got = channel_actions::strip_signoff_for(
            c["text"].as_str().unwrap(),
            c["closing"].as_bool().unwrap(),
            "Acme Ltd",
        );
        // Fix batch 1 (B6) compares closings case- and punctuation-insensitively ("Kind  regards",
        // "BEST REGARDS!"), so a closing Python kept at the end goes too when the signature has its own.
        let want = if c["closing"].as_bool().unwrap() {
            channel_actions::strip_signoff_for(c["out"].as_str().unwrap(), true, "Acme Ltd")
        } else {
            c["out"].as_str().unwrap().to_string()
        };
        assert_eq!(got, want, "{c}");
    }
    assert!(cases.len() >= 1000);
}

#[test]
fn golden_html() {
    for c in load("html") {
        assert_eq!(
            channel_actions::_html(c["text"].as_str().unwrap()),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn golden_steps_and_schedule() {
    let where_ = cadence::WHERE;
    let key = "nary.cadence.steps";
    let (mut ok, mut err, mut over) = (0, 0, 0);
    for c in load("steps") {
        let raw = c["raw"].as_str().unwrap().to_string();
        let vals: HashMap<String, Option<String>> =
            [(key.to_string(), Some(raw))].into_iter().collect();
        let got = require_from(&vals, key, where_)
            .and_then(|r| parse_json(&r, key, where_))
            .map_err(ApiError::from)
            .and_then(|obj| cadence::check_steps(obj.to_json()));
        match (&got, c.get("error")) {
            (Err(e), Some(want)) => {
                let status = want["status"].as_u64().unwrap();
                assert_eq!(e.status().as_u16() as u64, status, "{c}");
                if status == 400 {
                    assert_eq!(e.body()["detail"], want["detail"], "{c}");
                }
                err += 1;
            }
            (Ok(steps), None) => {
                assert_eq!(&Value::Array(steps.clone()), &c["steps"], "{c}");
                let base = DateTime::parse_from_rfc3339(
                    c["base"].as_str().unwrap_or("2026-10-15T03:04:05+00:00"),
                )
                .unwrap()
                .with_timezone(&Utc);
                // (an overflow case records no base: any base overflows the same)
                match (cadence::schedule(steps, base), c.get("schedule")) {
                    (Ok(rows), Some(want)) => {
                        let rows: Vec<Value> = rows
                            .iter()
                            .map(|r| {
                                json!({"day": r.day, "channel": r.channel, "intent": r.intent, "topic": r.topic,
                                       "due_at": cadence::iso(&r.due_at)})
                            })
                            .collect();
                        assert_eq!(&Value::Array(rows), want, "{c}");
                        ok += 1;
                    }
                    (Err(_), None) => {
                        assert!(c.get("schedule_error").is_some(), "{c}");
                        over += 1;
                    }
                    (got, _) => panic!("schedule mismatch {got:?} vs {c}"),
                }
            }
            (got, _) => panic!("mismatch {got:?} vs {c}"),
        }
    }
    assert!(ok > 20 && err > 50 && over > 0, "{ok} {err} {over}");
}

#[test]
fn golden_draft_request() {
    let mut n = 0;
    let mut cases = load("draft");
    for c in cases.iter_mut() {
        let channel = c["channel"].as_str().unwrap().to_string();
        let channel = channel.as_str();
        let q = &c["quotation"].clone();
        let number = match opt(&q["full_number"]).filter(|s| !s.is_empty()) {
            Some(f) => f.to_string(),
            None => q["number"].as_str().unwrap().to_string(),
        };
        let valid_until =
            NaiveDate::parse_from_str(q["valid_until"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        let english = cadence::draft_english(
            opt(&c["conv_language"]),
            Some(&c["lead_ai_score"]).filter(|v| !v.is_null()),
        );
        let day = c["step"]["day"].as_i64().unwrap() as i32;
        let intent_s = opt(&c["step"]["intent"]).map(String::from);
        let intent = intent_s.as_deref();
        let rules = cadence::draft_rules(
            day,
            intent,
            &number,
            valid_until,
            english,
            channel,
            c["max_lines"].as_i64().unwrap(),
        );
        let total = opt(&q["total"]).map(|t| t.parse::<rust_decimal::Decimal>().unwrap());
        let content = cadence::draft_content(
            &number,
            total,
            opt(&q["currency"]),
            valid_until,
            opt(&c["contact_first_name"]),
            c["transcript"].as_str().unwrap(),
        )
        .unwrap();
        let system = persona::system_prompt("customer", "", c["style"].as_str().unwrap(), &rules);
        let msg = format!("draft case {n}");
        {
            let seen = &mut c["seen"];
            bless::eq(&mut seen["content"], content, &msg);
            bless::eq(&mut seen["system"], system, &msg);
            bless::eq(&mut seen["schema"], cadence::draft_schema(), &msg);
        }
        let seen = &c["seen"];
        assert_eq!(seen["job"], "nary.followup_draft");
        assert_eq!(seen["max_lines_key"], "nary.reply.max_lines");
        assert_eq!(seen["style_channel"], channel);
        // the card: proposed only with a usable text
        let text = c["answer"]
            .get("text")
            .and_then(|t| t.as_str())
            .map(|t| t.trim())
            .unwrap_or("");
        if text.is_empty() {
            assert!(seen.get("action").is_none());
            assert_eq!(c["status"], "skipped");
            assert_eq!(c["reason"], "the follow-up couldn't be drafted");
        } else {
            assert_eq!(seen["action"], "cadence.followup.send");
            assert!(seen["sop_ref"].is_null(), "{seen}");
            let rationale = format!(
                "Follow-up day {day} for {number}: {}",
                intent.unwrap_or("None")
            );
            assert_eq!(seen["rationale"].as_str().unwrap(), rationale);
            let p = &seen["payload"];
            assert_eq!(p["channel"], channel);
            assert_eq!(p["text"].as_str().unwrap(), text);
            assert_eq!(p["contact_id"], c["contact_id"]);
            assert_eq!(c["status"], "drafted");
            assert_eq!(c["ok"], c["card_made"]);
        }
        n += 1;
    }
    bless::save("nary_cadence/draft.json", &Value::Array(cases));
    assert!(n >= 100);
}

/// The card labels, with the rows they read written to a fresh database.
#[tokio::test]
async fn golden_describe_labels() {
    let pool = support::fresh_db("n11_cadence_golden").await;
    let mut conn = pool.acquire().await.unwrap();
    let db = &mut *conn;
    let mut n = 0;
    for c in load("describe") {
        let p = &c["p"];
        let got = match c["kind"].as_str().unwrap() {
            "text" => {
                let chat_id = p["chat_id"].as_str().unwrap();
                sqlx::query("DELETE FROM wa_chats WHERE chat_id = $1")
                    .bind(chat_id)
                    .execute(&mut *db)
                    .await
                    .unwrap();
                if let Some(chat) = c["chat"].as_object() {
                    sqlx::query(
                        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, \
                         attribution_asked) VALUES ($1, $2, $3, $4, false, 'active', true, false)",
                    )
                    .bind(Uuid::new_v4())
                    .bind(chat_id)
                    .bind(chat["phone"].as_str())
                    .bind(chat["display_name"].as_str())
                    .execute(&mut *db)
                    .await
                    .unwrap();
                }
                channel_actions::_describe_text(db, None, p).await.unwrap()
            }
            "email" => channel_actions::_describe_email(db, None, p).await.unwrap(),
            "identity" => {
                if let Some(link) = c["link"].as_object() {
                    sqlx::query(
                        "INSERT INTO nary_identity_links (id, subject_type, subject_ref, tier, score, signals, status, \
                         cascaded) VALUES ($1, 'wa_chat', $2, 'weak', 0, $3, 'pending', false)",
                    )
                    .bind(Uuid::parse_str(p["link_id"].as_str().unwrap()).unwrap())
                    .bind(link["subject_ref"].as_str().unwrap())
                    .bind(link["signals"].clone())
                    .execute(&mut *db)
                    .await
                    .unwrap();
                }
                channel_actions::_describe_identity(db, None, p)
                    .await
                    .unwrap()
            }
            other => panic!("{other}"),
        };
        assert_eq!(got, c["out"], "{c}");
        n += 1;
    }
    assert!(n >= 200);
}
