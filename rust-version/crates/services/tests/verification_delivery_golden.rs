//! Golden cases recorded from the real Python functions by
//! compat-tests/golden/gen_verification_delivery.py: delivery `_filename`, `_send_key` and signing_flow
//! `_place_fields`. The send keys re-record with `GOLDEN_BLESS=1` (tests/golden/bless.rs).

#[path = "golden/bless.rs"]
mod bless;

use std::path::PathBuf;

use granary_services::verification::{delivery, signing_flow};
use rust_decimal::Decimal;
use serde_json::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compat-tests/golden/verification_delivery")
}

fn cases() -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join("cases.json")).unwrap()).unwrap()
}

#[test]
fn filename_matches_python() {
    for c in cases()["filename"].as_array().unwrap() {
        assert_eq!(
            delivery::_filename(c[0].as_str().unwrap()),
            c[1].as_str().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn send_key_matches_python() {
    let mut all = cases();
    for c in all["send_key"].as_array_mut().unwrap() {
        let to: Vec<String> = c[3]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let key = delivery::send_key_for_id(
            c[0].as_str().unwrap(),
            c[1].as_str().unwrap(),
            c[2].as_str().unwrap(),
            &to,
        );
        let msg = c.to_string();
        bless::eq(&mut c[4], key, msg);
    }
    let send_key = all["send_key"].clone();
    bless::update("verification_delivery/cases.json", |v| {
        v["send_key"] = send_key
    });
}

fn dec(v: &Value) -> Decimal {
    match v {
        Value::String(s) => s.parse().unwrap(),
        Value::Number(n) => n.to_string().parse().unwrap(),
        _ => panic!("{v}"),
    }
}

#[test]
fn place_fields_matches_python() {
    for c in cases()["place_fields"].as_array().unwrap() {
        let pdf = std::fs::read(dir().join(c["pdf"].as_str().unwrap())).unwrap();
        let anchors: Vec<&str> = c["anchors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let placed = signing_flow::_place_fields(&pdf, &anchors).unwrap();
        let want = c["placed"].as_object().unwrap();
        assert_eq!(placed.len(), want.len(), "{c}");
        for (order, w) in want {
            let p = &placed[&order.parse::<i32>().unwrap()];
            assert_eq!(
                p.page_number as i64,
                w["page_number"].as_i64().unwrap(),
                "{c}"
            );
            for (k, got) in [
                ("x", p.x),
                ("y", p.y),
                ("width", p.width),
                ("height", p.height),
            ] {
                assert_eq!(got.normalize(), dec(&w[k]).normalize(), "{k} in {c}");
            }
        }
    }
}
