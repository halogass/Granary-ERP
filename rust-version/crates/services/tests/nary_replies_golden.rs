//! Golden fixtures recorded from backend/app/services/nary/{replies,reply_alerts,ack,handoff}.py
//! (compat-tests/golden/gen_nary_replies.py): the pure parts.

use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
use granary_db::rows::crm::{Contact, Deal, Lead};
use granary_db::rows::nary::{AgentPendingAction, WaChat};
use granary_services::nary::replies::{Amount, Conversation};
use granary_services::nary::{ack, handoff, replies, reply_alerts};
use serde_json::{json, Value};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/nary_replies/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

fn row<T: serde::de::DeserializeOwned>(base: Value, extra: &Value) -> T {
    let mut base = base;
    if let Value::Object(m) = extra {
        for (k, v) in m {
            base[k] = v.clone();
        }
    }
    serde_json::from_value(base).expect("row")
}

const TS: &str = "2026-10-15T00:00:00Z";

#[test]
fn golden_amount() {
    for c in load("amount").as_array().unwrap() {
        let got = match replies::_amount(&c["value"]) {
            None => Value::Null,
            Some(Amount::NaN) => json!("NaN"),
            Some(Amount::Infinite(neg)) => json!(if neg { "-Inf" } else { "Inf" }),
            Some(Amount::Finite(neg, digits, exp)) => json!([neg, digits, exp]),
        };
        assert_eq!(got, c["key"], "{:?}", c["value"]);
    }
}

#[test]
fn golden_never_in_price() {
    for c in load("never_in_price").as_array().unwrap() {
        let t = c["text"].as_str().unwrap();
        assert_eq!(
            replies::_NEVER_IN_PRICE.is_match(t),
            c["match"].as_bool().unwrap(),
            "{t}"
        );
    }
}

#[test]
fn golden_card_line() {
    for c in load("card_line").as_array().unwrap() {
        let card: Option<AgentPendingAction> = if c["card"].is_null() {
            None
        } else {
            Some(row(
                json!({"action": "whatsapp.reply.send", "origin": "nary", "level": 1, "customer_facing": true,
                       "payload": {}, "id": "11111111-2222-3333-4444-555555555555", "created_at": TS, "updated_at": TS}),
                &c["card"],
            ))
        };
        assert_eq!(
            reply_alerts::_card_line(card.as_ref()),
            c["line"].as_str().unwrap(),
            "{:?}",
            c["card"]
        );
    }
}

#[test]
fn golden_conversation_payload_and_label() {
    for c in load("conversation").as_array().unwrap() {
        let mut conv = Conversation::new(
            if c["chat"].is_null() {
                "email"
            } else {
                "whatsapp"
            },
            "",
        );
        if !c["chat"].is_null() {
            conv.chat = Some(row::<WaChat>(
                json!({"is_my_contact": false, "status": "active", "attribution_asked": false,
                       "id": "11111111-2222-3333-4444-555555555556", "created_at": TS, "updated_at": TS}),
                &c["chat"],
            ));
        }
        if !c["contact"].is_null() {
            conv.contact = Some(row::<Contact>(
                json!({"first_name": "Bob", "custom": {}, "created_at": TS, "updated_at": TS, "is_deleted": false}),
                &c["contact"],
            ));
        }
        if !c["lead"].is_null() {
            conv.lead = Some(row::<Lead>(
                json!({"source": "whatsapp", "custom": {}, "id": "11111111-2222-3333-4444-555555555557",
                       "created_at": TS, "updated_at": TS, "is_deleted": false}),
                &c["lead"],
            ));
        }
        if !c["deal"].is_null() {
            conv.deal = Some(row::<Deal>(
                json!({"stage": "qualification", "custom": {}, "id": "11111111-2222-3333-4444-555555555558",
                       "created_at": TS, "updated_at": TS, "is_deleted": false}),
                &c["deal"],
            ));
        }
        conv.thread_id = c["thread"].as_str().map(|t| t.parse().unwrap());
        let payload = conv.reply_payload("Hello, thank you");
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            serde_json::to_string(&c["payload"]).unwrap(),
            "{c}"
        );
        assert_eq!(conv.label(), c["label"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn golden_format_named() {
    for c in load("format_named").as_array().unwrap() {
        let t = c["template"].as_str().unwrap();
        let got = ack::format_named(t, &[("name", "Bob"), ("reply_window", "1 hour")]);
        match c.get("out") {
            Some(out) => assert_eq!(got.unwrap(), out.as_str().unwrap(), "{t}"),
            None => assert!(got.is_err(), "{t}"),
        }
    }
}

fn py_dumps(v: &Value) -> String {
    dumps(&PyValue::from(v), &DumpOpts::default()).unwrap()
}

#[test]
fn golden_constants() {
    let c = load("constants");
    assert_eq!(
        py_dumps(&replies::REPLY_SCHEMA),
        c["reply_schema"].as_str().unwrap()
    );
    // fix batch 1 (C1): the nullable enum is anyOf now (a type list with an enum is a 400 at Anthropic)
    assert_eq!(
        py_dumps(&replies::FIT_SCHEMA),
        c["fit_schema"].as_str().unwrap().replace(
            r#"{"type": ["string", "null"], "enum": ["custom", "tender", "complex", "asked", null]}"#,
            r#"{"anyOf": [{"type": "string", "enum": ["custom", "tender", "complex", "asked"]}, {"type": "null"}]}"#
        )
    );
    assert_eq!(
        py_dumps(&replies::PRICE_SCHEMA),
        c["price_schema"].as_str().unwrap()
    );
    assert_eq!(
        py_dumps(&reply_alerts::SCHEMA),
        c["alert_schema"].as_str().unwrap()
    );
    assert_eq!(reply_alerts::SYSTEM, c["alert_system"].as_str().unwrap());
    assert_eq!(reply_alerts::JOB, c["alert_job"].as_str().unwrap());
    assert_eq!(
        py_dumps(&handoff::HANDOFF_SCHEMA),
        c["handoff_schema"].as_str().unwrap()
    );
    assert_eq!(
        py_dumps(&handoff::LOST_SCHEMA),
        c["lost_schema"].as_str().unwrap()
    );
    assert_eq!(json!(handoff::SECTIONS), c["sections"]);
    assert_eq!(json!(ack::TEMPLATE_KEYS), c["template_keys"]);
}
