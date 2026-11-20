//! Golden replay of crm/followups.py's pure parts (fixtures from compat-tests/golden/gen_crm_followups.py,
//! recorded from the real Python functions).

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use granary_db::rows::crm::CrmReminder;
use granary_services::crm::followups::{
    default_due, digest_message, in_quiet_hours, link_for, FollowupSettings,
};
use serde_json::{json, Map, Value};

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../compat-tests/golden/crm_followups/{name}.json"
    ));
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
}

fn cfg_of(v: &Value) -> FollowupSettings {
    let mut cfg = FollowupSettings::default();
    let raw: HashMap<String, String> = v
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => {
                        if *b {
                            "true".into()
                        } else {
                            "false".into()
                        }
                    }
                    other => other.to_string(),
                },
            )
        })
        .collect();
    cfg.apply_raw(&raw).unwrap();
    cfg
}

fn at(v: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(v.as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn golden_settings() {
    for c in fixture("settings").as_array().unwrap() {
        let mut cfg = FollowupSettings::default();
        let raw: HashMap<String, String> = c["raw"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect();
        cfg.apply_raw(&raw).unwrap();
        assert_eq!(serde_json::to_value(&cfg).unwrap(), c["cfg"], "case {c}");
    }
}

#[test]
fn golden_quiet_and_due() {
    let mut quiet = fixture("quiet");
    for c in quiet.as_array_mut().unwrap() {
        let tz: chrono_tz::Tz = c["tz"].as_str().unwrap().parse().unwrap();
        let got = in_quiet_hours(&cfg_of(&c["cfg"]), tz, Some(at(&c["at"])));
        if bless::blessing() {
            c["out"] = json!(got);
        } else {
            assert_eq!(got, c["out"].as_bool().unwrap(), "case {c}");
        }
    }
    bless::save("crm_followups/quiet.json", &quiet);
    let mut due = fixture("due");
    for c in due.as_array_mut().unwrap() {
        let tz: chrono_tz::Tz = c["tz"].as_str().unwrap().parse().unwrap();
        let got = default_due(&cfg_of(&c["cfg"]), tz, Some(at(&c["at"]))).ok();
        if bless::blessing() {
            c["out"] = json!(got.map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)));
            continue;
        }
        let want = c["out"]
            .as_str()
            .map(|s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
        assert_eq!(got, want, "case {c}");
    }
    bless::save("crm_followups/due.json", &due);
}

#[test]
fn golden_links() {
    for c in fixture("links").as_array().unwrap() {
        let id = c["entity_id"].as_str().map(|s| s.parse().unwrap());
        assert_eq!(
            link_for(
                c["entity_type"].as_str().unwrap(),
                id,
                c["chat_id"].as_str()
            ),
            c["out"].as_str().unwrap()
        );
    }
}

fn reminder(item: &Value) -> CrmReminder {
    let mut m: Map<String, Value> = Map::new();
    for (k, v) in [
        (
            "owner_user_id",
            json!("00000000-0000-0000-0000-000000000001"),
        ),
        ("source", json!("manual")),
        ("due_at", json!("2026-01-01T00:00:00Z")),
        ("status", json!("open")),
        ("snooze_count", json!(0)),
        ("id", json!("00000000-0000-0000-0000-000000000002")),
        ("created_at", json!("2026-01-01T00:00:00Z")),
        ("updated_at", json!("2026-01-01T00:00:00Z")),
    ] {
        m.insert(k.into(), v);
    }
    for (k, v) in item.as_object().unwrap() {
        m.insert(k.clone(), v.clone());
    }
    serde_json::from_value(Value::Object(m)).unwrap()
}

#[test]
fn golden_digest() {
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgresql+asyncpg://granary:granarytest@127.0.0.1:55433/unused",
        );
    }
    for c in fixture("digest").as_array().unwrap() {
        assert_eq!(
            granary_core::config::settings().APP_URL,
            c["app_url"].as_str().unwrap()
        );
        let items: Vec<CrmReminder> = c["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(reminder)
            .collect();
        let got = digest_message(&items, c["cap"].as_i64().unwrap());
        assert_eq!(serde_json::to_value(&got).unwrap(), c["out"], "case {c}");
    }
}
