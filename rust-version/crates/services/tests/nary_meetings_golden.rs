//! Replays compat-tests/golden/nary_meetings/*.json (recorded from the REAL backend functions by
//! compat-tests/golden/gen_nary_meetings.py): slots.find over many calendars, time zones, DST changes,
//! Reduced-hours periods, holidays, busy times and booked days; Slot.as_dict; meetings.customer_tz for every country;
//! slots_message, with_link and transcript_text. Rust is now the reference: re-record with GOLDEN_BLESS=1 after
//! an intended change and review the diff.

#[path = "golden/bless.rs"]
mod bless;

use std::collections::HashMap;
use std::path::PathBuf;

use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use granary_services::nary::{meetings, slots};
use serde_json::{json, Value};

fn golden(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compat-tests/golden/nary_meetings")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn slots_find_matches_python() {
    let mut cases = golden("slots.json");
    let mut failures = Vec::new();
    for (i, c) in cases.as_array_mut().unwrap().iter_mut().enumerate() {
        let busy: Vec<_> = c["busy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    DateTime::parse_from_rfc3339(p[0].as_str().unwrap()).unwrap(),
                    DateTime::parse_from_rfc3339(p[1].as_str().unwrap()).unwrap(),
                )
            })
            .collect();
        let booked: Option<HashMap<NaiveDate, i64>> = c["booked"].as_object().map(|o| {
            o.iter()
                .map(|(k, v)| (k.parse().unwrap(), v.as_i64().unwrap()))
                .collect()
        });
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(c["now"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let company_tz = c["company_tz"].as_str().unwrap();
        let customer_tz = c["customer_tz"].as_str();
        let got = slots::find(
            &c["rules"],
            &c["holidays"],
            &c["reduced_hours"],
            &c["customer_hours"],
            company_tz,
            customer_tz,
            c["country"].as_str(),
            &busy,
            now,
            booked.as_ref(),
        );
        let got = got.map(|found| {
            found
                .iter()
                .map(|s| s.as_dict(company_tz, customer_tz).unwrap())
                .collect::<Vec<Value>>()
        });
        if bless::blessing() {
            match &got {
                Ok(dicts) => c["result"] = json!({"slots": dicts}),
                Err(_) if c["result"].get("error").is_some() => {}
                Err(_) => c["result"] = json!({"error": "Error"}),
            }
            continue;
        }
        let want = &c["result"];
        match (got, want.get("slots")) {
            (Ok(dicts), Some(w)) => {
                if &Value::Array(dicts.clone()) != w {
                    failures.push(format!("case {i}: got {dicts:?}\nwant {w}"));
                }
            }
            (Err(_), None) => {}
            (g, _) => failures.push(format!(
                "case {i}: got {:?}, want {want}",
                g.map(|v| v.len())
            )),
        }
    }
    bless::save("nary_meetings/slots.json", &cases);
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn customer_tz_matches_python() {
    let mut cases = golden("customer_tz.json");
    for c in cases.as_array_mut().unwrap() {
        let got = meetings::customer_tz(c["country"].as_str(), c["company_tz"].as_str().unwrap());
        let msg = c.to_string();
        bless::eq(&mut c["result"], got, msg);
    }
    bless::save("nary_meetings/customer_tz.json", &cases);
}

#[test]
fn texts_match_python() {
    let g = golden("texts.json");
    let t = &g["slots_message"]["t"];
    for c in g["slots_message"]["cases"].as_array().unwrap() {
        let list = c["slots"].as_array().unwrap();
        assert_eq!(
            meetings::slots_message(list, t),
            c["result"].as_str().unwrap(),
            "{c}"
        );
    }
    for c in g["with_link"].as_array().unwrap() {
        assert_eq!(
            meetings::with_link(c["text"].as_str().unwrap(), c["link"].as_str()),
            c["result"].as_str().unwrap(),
            "{c}"
        );
    }
    for c in g["transcript_text"].as_array().unwrap() {
        let data = base64::engine::general_purpose::STANDARD
            .decode(c["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            meetings::transcript_text(c["filename"].as_str().unwrap(), &data).unwrap(),
            c["result"].as_str().unwrap(),
            "{c}"
        );
    }
}
