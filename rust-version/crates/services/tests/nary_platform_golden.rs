//! Golden cases for the nary platform's pure parts (agent N1), recorded from the real Python
//! (compat-tests/golden/gen_nary_platform.py): action payload schemas, plain_error, error-card fingerprints and
//! keys, autonomy.edit_ratio, the never-promote fnmatch, llm.information. Schemas, fingerprints, error-card keys
//! and edit ratios re-record with GOLDEN_BLESS=1 (Rust is now the reference; review the diff).

#[path = "golden/bless.rs"]
mod bless;

use granary_services::nary::{actions, autonomy, executor, governance, llm};
use serde_json::Value;

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/nary_platform/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

#[test]
fn golden_schemas() {
    let mut cases = load("schemas");
    let builtins = actions::builtin_actions();
    let mut bad = vec![];
    if bless::blessing() {
        for c in cases.iter_mut() {
            let a = builtins.iter().find(|a| a.name == s(&c["action"])).unwrap();
            let o = c.as_object_mut().unwrap();
            match a.schema.validate(&o["payload"]) {
                Ok(v) => {
                    o.remove("error");
                    o.insert("ok".into(), v);
                }
                Err(e) => {
                    o.remove("ok");
                    o.insert("error".into(), Value::String(format!("{}: {e}", a.name)));
                }
            }
        }
        bless::write("nary_platform/schemas.json", &Value::Array(cases.clone()));
    }
    for c in &cases {
        let a = builtins.iter().find(|a| a.name == s(&c["action"])).unwrap();
        let got = a.schema.validate(&c["payload"]);
        let ok = match (&got, c.get("ok"), c.get("error")) {
            (Ok(v), Some(exp), _) => {
                v == exp && serde_json::to_string(v).unwrap() == serde_json::to_string(exp).unwrap()
            }
            (Err(e), _, Some(exp)) => format!("{}: {e}", a.name) == s(exp),
            _ => false,
        };
        if !ok {
            bad.push(format!("{c}\n  rust: {got:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} differ:\n{}",
        bad.len(),
        cases.len(),
        bad.iter().take(8).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn golden_plain_error() {
    for c in load("plain_error") {
        assert_eq!(
            governance::plain_error_text(s(&c["type"]), s(&c["text"])),
            s(&c["plain"]),
            "{c}"
        );
    }
}

#[test]
fn golden_fingerprints_and_keys() {
    let mut fps = Value::Array(load("fingerprint"));
    for c in fps.as_array_mut().unwrap() {
        let parts: Vec<&str> = c["parts"].as_array().unwrap().iter().map(s).collect();
        let got = executor::_fingerprint(&parts);
        let msg = c.to_string();
        bless::eq(&mut c["fp"], got, msg);
    }
    bless::save("nary_platform/fingerprint.json", &fps);
    let mut keys = Value::Array(load("error_card_keys"));
    for c in keys.as_array_mut().unwrap() {
        let a = s(&c["action"]);
        let key = match s(&c["kind"]) {
            "action" => {
                let err = granary_services::nary::py_slice(
                    granary_services::nary::py_slice(s(&c["error"]), 2000),
                    80,
                );
                format!("action:{a}:{}", executor::_fingerprint(&[err]))
            }
            "send" => format!(
                "send:{a}:{}",
                executor::_fingerprint(&[s(&c["who"]), s(&c["reason"])])
            ),
            _ => format!("chat:{a}:{}", executor::_fingerprint(&[s(&c["reason"])])),
        };
        let msg = c.to_string();
        bless::eq(&mut c["key"], key, msg);
    }
    bless::save("nary_platform/error_card_keys.json", &keys);
}

#[test]
fn golden_edit_ratio() {
    let mut cases = load("edit_ratio");
    for c in cases.iter_mut() {
        let edited = if c["edited"].is_null() {
            None
        } else {
            Some(&c["edited"])
        };
        let got = autonomy::edit_ratio(&c["original"], edited);
        if bless::blessing() {
            c["ratio"] = Value::String(got.to_string());
            continue;
        }
        let exp = s(&c["ratio"]);
        let e: rust_decimal::Decimal = exp.parse().unwrap();
        assert!(got == e && got.scale() == e.scale(), "{c}: rust {got}");
    }
    bless::save("nary_platform/edit_ratio.json", &Value::Array(cases));
}

#[test]
fn golden_fnmatch() {
    for c in load("fnmatch") {
        let got = autonomy::_matches(s(&c["name"]), &[s(&c["pattern"]).to_string()]);
        assert_eq!(got, c["match"].as_bool().unwrap(), "{c}");
    }
}

#[test]
fn golden_information() {
    for c in load("information") {
        let got = llm::information(
            s(&c["source"]),
            s(&c["text"]),
            c["limit"].as_u64().unwrap() as usize,
        );
        assert_eq!(got, s(&c["out"]), "{c}");
    }
}
