//! Golden replay on the scenario database (nary_replies/scenario.sql), with fixed steps and model answers:
//! every model request (job, system prompt, user content) must be byte-identical to the recording, and the
//! results equal. First recorded from the retired Python code; re-record with GOLDEN_BLESS=1 after an
//! intended change and review the diff.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use chrono::{DateTime, Duration, Utc};
use granary_db::rows::crm::{Deal, Lead};
use granary_db::rows::crm_inbox::CrmEmailThread;
use granary_db::rows::nary::{NaryInsight, NaryOwnerTopic, WaChat, WaMessage};
use granary_services::ai::base::AiError;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::{ack, handoff, replies, reply_alerts};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

fn golden(name: &str) -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/nary_replies/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

/// The writing check before a card asks the model too: the replay runs without it.
const NO_LINT: &str =
    "UPDATE system_settings SET value = 'false' WHERE key = 'nary.writing.lint_enabled';";

const CHAT: &str = "15550123456@c.us";
const T_CHAT: &str = "Customer: Hello, I have a question about the laptop kit\nAcme: Here is the meeting link for tomorrow\nCustomer: This is my email";
const T_MAIL: &str = "Mary: The Springfield warehouse, 3 floors. Can you quote?";

fn id(tail: &str) -> Uuid {
    format!("00000000-0000-0000-0000-00000000{tail}")
        .parse()
        .unwrap()
}

fn fit() -> Value {
    json!({"summary": "Laptops for 15 staff", "requirements": [{"requirement": "single sign-on", "status": "covered",
                                                               "how": "Office suite"}],
           "open_questions": ["Docking station brand?"], "proposal_trigger": null, "suggested_products": ["Laptop kit"]})
}

fn card_out(out: Option<Value>) -> Value {
    match out {
        None => Value::Null,
        Some(Value::Object(mut m)) => {
            m.remove("card_id");
            Value::Object(m)
        }
        Some(v) => v,
    }
}

struct Steps {
    brain: Brain,
    expected: Vec<Value>,
    i: usize,
}

impl Steps {
    /// Compares the calls since the last step and the step's results with the recording (or records them when
    /// blessing).
    fn check(&mut self, name: &str, out: Value, extra: &[(&str, Value)]) {
        let calls = self.brain.calls();
        self.brain.clear();
        let got_calls: Vec<Value> = calls
            .iter()
            .map(|(job, call)| {
                let req = call.chat().unwrap();
                assert_eq!(req.messages.len(), 1, "{name}");
                json!({"job": job, "system": req.system, "content": req.messages[0]["content"]})
            })
            .collect();
        if bless::blessing() {
            let mut rec = json!({"step": name, "calls": got_calls, "out": out});
            for (k, v) in extra {
                rec[*k] = v.clone();
            }
            if self.i < self.expected.len() {
                self.expected[self.i] = rec;
            } else {
                self.expected.push(rec);
            }
            self.i += 1;
            return;
        }
        let want = &self.expected[self.i];
        self.i += 1;
        assert_eq!(want["step"], name);
        let want_calls = want["calls"].as_array().unwrap();
        assert_eq!(
            got_calls.len(),
            want_calls.len(),
            "{name}: number of model calls"
        );
        for (g, w) in got_calls.iter().zip(want_calls) {
            assert_eq!(g["job"], w["job"], "{name}");
            assert_eq!(g["system"], w["system"], "{name}: system prompt");
            assert_eq!(g["content"], w["content"], "{name}: content");
        }
        assert_eq!(out, want["out"], "{name}: result");
        for (k, v) in extra {
            assert_eq!(v, &want[*k], "{name}: {k}");
        }
    }
}

async fn row<T>(db: &mut PgConnection, sql: &str, id: Uuid) -> T
where
    T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    sqlx::query_as::<_, T>(sql)
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn golden_prompts_and_results() {
    let pool = support::fresh_db("n10_prompts").await;
    bless::neutral_owner(&pool).await;
    granary_services::nary::set_session_pool(pool.clone());
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let sql = std::fs::read_to_string(format!(
        "{}/../../compat-tests/golden/nary_replies/scenario.sql",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    sqlx::raw_sql(&sql).execute(&mut *db).await.unwrap();
    sqlx::raw_sql(NO_LINT).execute(&mut *db).await.unwrap();
    let brain = Brain::new();
    let _seam = brain.install().await;
    let mut s = Steps {
        brain: brain.clone(),
        expected: golden("prompts.json").as_array().unwrap().clone(),
        i: 0,
    };

    let chat: WaChat = sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(CHAT)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let mut conv = replies::for_chat(db, &chat, T_CHAT).await.unwrap();
    s.check(
        "for_chat",
        json!({"country": conv.country, "currency": conv.currency, "language": conv.language, "label": conv.label()}),
        &[],
    );

    brain.set(
        "nary.reply_draft",
        Answer::json(json!({"reply": " Sure John, we can. How did you hear about us? ",
                            "asked": ["how they found us"], "customer_said_how_found": "Instagram ads"})),
    );
    let out = replies::qualifying_reply(
        db,
        &mut conv,
        Some("software"),
        None,
        "other",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let lead: Lead = row(db, "SELECT * FROM leads WHERE id = $1", id("a003")).await;
    s.check(
        "reply_other",
        card_out(out),
        &[("attribution", lead.attribution.clone().unwrap())],
    );

    brain.set(
        "nary.reply_draft",
        Answer::json(
            json!({"reply": "We understand.", "asked": [], "customer_said_how_found": null}),
        ),
    );
    let disc = json!({"ask": ["Docking station brand?", "site count?"], "ready": false});
    let out = replies::qualifying_reply(
        db,
        &mut conv,
        Some("nope"),
        Some(&fit()),
        "objection",
        None,
        Some(&disc),
        None,
    )
    .await
    .unwrap();
    s.check("reply_objection", card_out(out), &[]);

    let disc = json!({"ask": [], "ready": true});
    let out =
        replies::qualifying_reply(db, &mut conv, None, None, "accept", None, Some(&disc), None)
            .await
            .unwrap();
    s.check("reply_accept", card_out(out), &[]);

    brain.set(
        "nary.reply_draft",
        Answer::json(json!({"reply": "", "asked": [], "customer_said_how_found": null})),
    );
    let out = replies::qualifying_reply(db, &mut conv, None, None, "meeting", None, None, None)
        .await
        .unwrap();
    s.check("reply_empty", card_out(out), &[]);

    let thread: CrmEmailThread = row(
        db,
        "SELECT * FROM crm_email_threads WHERE id = $1",
        id("a007"),
    )
    .await;
    let mut conv_e = replies::for_email(db, &thread, T_MAIL).await.unwrap();
    brain.set(
        "nary.reply_draft",
        Answer::json(json!({"reply": "Dear Mary, we are preparing it.", "asked": ["floors", "brand"], "customer_said_how_found": null})),
    );
    let ask = vec!["number of floors".to_string(), "laptop brand".to_string()];
    let out = replies::qualifying_reply(
        db,
        &mut conv_e,
        Some("laptops"),
        None,
        "quotation",
        None,
        None,
        Some(&ask),
    )
    .await
    .unwrap();
    let conv_info = json!({"country": conv_e.country, "currency": conv_e.currency, "language": conv_e.language,
                           "label": conv_e.label()});
    s.check(
        "reply_email_quotation",
        card_out(out),
        &[("conv", conv_info)],
    );

    brain.set(
        "nary.reply_draft",
        Answer::json(json!({
            "reply": "Globex Laptop Kit: USD 1,000 per unit, 1 year warranty. For how many sites?",
            "quoted": [{"sku": "GLX-KIT", "price": "1,000", "currency": "USD"},
                       {"sku": "GLX-KIT", "price": "900", "currency": "USD"},
                       {"sku": "OTHER", "price": "1", "currency": "USD"}]})),
    );
    let out = replies::price_answer(db, &conv, Some("laptops"), None)
        .await
        .unwrap();
    let mem: String =
        sqlx::query_scalar("SELECT content FROM nary_memory WHERE title = 'Prices stated'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    s.check("price_answer", card_out(out), &[("memory", json!(mem))]);

    brain.set(
        "nary.reply_draft",
        Answer::json(json!({"reply": "USD 1,000, with a volume discount.", "quoted": []})),
    );
    let out = replies::price_answer(db, &conv, Some("laptops"), None)
        .await
        .unwrap();
    s.check("price_blocked", card_out(out), &[]);
    let out = replies::price_answer(db, &conv, Some("nothing_matches"), None)
        .await
        .unwrap();
    s.check("price_no_catalog", card_out(out), &[]);

    let mut f = fit();
    f["proposal_trigger"] = json!("complex");
    brain.set("nary.fit_check", Answer::json(f));
    let out = replies::fit_check(db, &conv, None).await.unwrap();
    let ins: NaryInsight = sqlx::query_as("SELECT * FROM nary_insights WHERE kind = 'fit_check'")
        .fetch_one(&mut *db)
        .await
        .unwrap();
    s.check(
        "fit_check",
        out.unwrap(),
        &[
            ("model", json!(ins.model)),
            ("lead_id", json!(ins.lead_id.map(|l| l.to_string()))),
        ],
    );

    let msg: WaMessage = row(db, "SELECT * FROM wa_messages WHERE id = $1", id("b003")).await;
    brain.set(
        "nary.reply_alert",
        Answer::json(
            json!({"summary": " Gave her email for the invite. ", "needs_action": true,
                            "action": " Add john@globex.test to the invite "}),
        ),
    );
    let alert = reply_alerts::on_whatsapp(db, &chat, &msg, None)
        .await
        .unwrap()
        .unwrap();
    let topic: Option<NaryOwnerTopic> = match alert.topic_id {
        Some(t) => Some(row(db, "SELECT * FROM nary_owner_topics WHERE id = $1", t).await),
        None => None,
    };
    s.check(
        "alert_whatsapp",
        json!({"who": alert.who, "summary": alert.summary, "needs_action": alert.needs_action, "action": alert.action,
               "has_card": alert.card_id.is_some(),
               "topic": topic.map(|t| json!({"kind": t.kind, "tag": t.tag, "text": t.text, "facts": t.facts,
                                             "numbered": t.numbered}))}),
        &[],
    );

    brain.set("nary.reply_alert", Answer::fail(|| AiError::ai("no model")));
    let alert = reply_alerts::on_email(db, &thread, None)
        .await
        .unwrap()
        .unwrap();
    let topic: Option<NaryOwnerTopic> = match alert.topic_id {
        Some(t) => Some(row(db, "SELECT * FROM nary_owner_topics WHERE id = $1", t).await),
        None => None,
    };
    s.check(
        "alert_email",
        json!({"who": alert.who, "summary": alert.summary, "needs_action": alert.needs_action, "action": alert.action,
               "has_card": alert.card_id.is_some(),
               "topic": topic.map(|t| {
                   let mut facts = t.facts.clone().unwrap();
                   facts["thread_id"] = json!("THREAD");
                   json!({"kind": t.kind, "tag": t.tag, "text": t.text, "facts": facts})
               })}),
        &[],
    );
    let again = reply_alerts::on_email(db, &thread, None).await.unwrap();
    s.check("alert_email_again", json!(again.is_none()), &[]);

    brain.set(
        "nary.handoff",
        Answer::json(json!({"summary": "15 laptops", "scope_sold": ["Laptops"], "scope_not_sold": ["printers"],
                            "success_criteria": [{"quote": "1 minute", "source": "WA"}], "commitments": [],
                            "sensitivities": [], "red_flags": ["no IT"], "contacts": [], "open_items": [],
                            "extra": "dropped"})),
    );
    let deal: Deal = row(db, "SELECT * FROM deals WHERE id = $1", id("a005")).await;
    let pkg = handoff::compile_package(db, &deal, None)
        .await
        .unwrap()
        .unwrap();
    s.check(
        "handoff",
        json!({"content": pkg.content, "model": pkg.model, "status": pkg.status}),
        &[],
    );

    brain.set("nary.lost_reply", Answer::json(json!({"reply": "Thank you Mary.", "analysis": "Budget moved.", "lesson": "Ask early."})));
    let deal: Deal = row(db, "SELECT * FROM deals WHERE id = $1", id("a008")).await;
    let out = handoff::on_lost(db, &deal).await.unwrap();
    let loss: NaryInsight = sqlx::query_as("SELECT * FROM nary_insights WHERE kind = 'loss'")
        .fetch_one(&mut *db)
        .await
        .unwrap();
    s.check(
        "lost",
        json!(out.is_some_and(|o| !o["card_id"].is_null())),
        &[("content", loss.content.clone())],
    );
    if bless::blessing() {
        s.expected.truncate(s.i);
        bless::write(
            "nary_replies/prompts.json",
            &Value::Array(s.expected.clone()),
        );
    }
    assert_eq!(s.i, s.expected.len());
}

#[tokio::test]
async fn golden_ack_compose() {
    let pool = support::fresh_db("n10_compose").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let sql = std::fs::read_to_string(format!(
        "{}/../../compat-tests/golden/nary_replies/scenario.sql",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    sqlx::raw_sql(&sql).execute(&mut *db).await.unwrap();
    sqlx::raw_sql(NO_LINT).execute(&mut *db).await.unwrap();
    let mut all = golden("compose.json");
    for c in all.as_array_mut().unwrap() {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(c["now"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let received = c["late"].as_i64().map(|m| now - Duration::minutes(m));
        let got = ack::compose(
            db,
            c["language"].as_str(),
            c["name"].as_str(),
            received,
            Some(now),
        )
        .await;
        if bless::blessing() {
            let o = c.as_object_mut().unwrap();
            for k in ["error", "lang", "text"] {
                o.shift_remove(k);
            }
            match got {
                Err(e) => {
                    o.insert("error".into(), e.body()["detail"].clone());
                }
                Ok((lang, text)) => {
                    o.insert("lang".into(), json!(lang));
                    o.insert("text".into(), json!(text));
                }
            }
            continue;
        }
        match c.get("error") {
            Some(err) => assert_eq!(got.unwrap_err().body()["detail"], *err, "{c}"),
            None => {
                let (lang, text) = got.unwrap();
                assert_eq!(lang, c["lang"].as_str().unwrap(), "{c}");
                assert_eq!(text, c["text"].as_str().unwrap(), "{c}");
            }
        }
    }
    bless::save("nary_replies/compose.json", &all);
}
