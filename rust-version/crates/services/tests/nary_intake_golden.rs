//! services/nary/{web3forms, intake, email_intake, triage} pure functions against
//! compat-tests/golden/nary_intake/*.json (first recorded from the retired Python modules; re-record with
//! GOLDEN_BLESS=1 after an intended change and review the diff). Object key order is compared too
//! (serialised text).
#[path = "golden/bless.rs"]
mod bless;

use std::collections::{BTreeSet, HashMap};

use base64::Engine;
use serde_json::{json, Map, Value};

use granary_services::imap_sync::pyemail::message_from_bytes;
use granary_services::nary::{email_intake, intake, triage, web3forms};

fn golden(name: &str) -> Value {
    bless::read(&format!("nary_intake/{name}.json"))
}

/// When blessing, writes the given keys of `g` into nary_intake/`name`.json.
fn save_keys(name: &str, g: &Value, keys: &[&str]) {
    bless::update(&format!("nary_intake/{name}.json"), |v| {
        for k in keys {
            v[*k] = g[*k].clone();
        }
    });
}

fn s(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn web3forms_header_auth_golden() {
    let mut g = golden("web3forms");
    let cases = g["header_auth"].as_array_mut().unwrap();
    assert!(cases.len() > 100);
    for c in cases {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(c["raw"].as_str().unwrap())
            .unwrap();
        let msg = message_from_bytes(&raw).unwrap();
        let tried = web3forms::try_header_auth(&msg);
        if bless::blessing() {
            c["out"] = match &tried {
                Ok(v) => json!({"ok": v}),
                Err(_) => json!({"err": true, "ok": web3forms::header_auth(&msg)}),
            };
            continue;
        }
        match tried {
            Ok(v) => assert_eq!(
                s(&v),
                s(&c["out"]["ok"]),
                "{}",
                String::from_utf8_lossy(&raw)
            ),
            Err(e) => assert!(c["out"].get("err").is_some(), "{e}"),
        }
        assert_eq!(s(&web3forms::header_auth(&msg)), s(&c["out"]["ok"]));
    }
    save_keys("web3forms", &g, &["header_auth"]);
}

#[test]
fn web3forms_parse_text_golden() {
    let g = golden("web3forms");
    for c in g["parse_text"].as_array().unwrap() {
        let p = web3forms::parse_text(opt(&c["text"]));
        let fields: Vec<Value> = p.fields.iter().map(|(k, v)| json!([k, v])).collect();
        assert_eq!(s(&Value::Array(fields)), s(&c["fields"]), "{:?}", c["text"]);
        assert_eq!(p.order, strs(&c["order"]), "{:?}", c["text"]);
        assert_eq!(p.loose, c["loose"].as_bool().unwrap(), "{:?}", c["text"]);
    }
}

#[test]
fn web3forms_check_golden() {
    let g = golden("web3forms");
    for c in g["check"].as_array().unwrap() {
        let p = web3forms::parse_text(opt(&c["text"]));
        let chk = web3forms::check(
            &p,
            opt(&c["reply_to"]),
            &strs(&c["known"]),
            &strs(&c["required"]),
            &c["routes"],
        );
        let o = &c["out"];
        assert_eq!(chk.missing, strs(&o["missing"]));
        assert_eq!(chk.unknown, strs(&o["unknown"]));
        assert_eq!(
            chk.reply_to_mismatch,
            o["reply_to_mismatch"].as_bool().unwrap()
        );
        assert_eq!(chk.source.as_deref(), o["source"].as_str());
        assert_eq!(chk.problems, strs(&o["problems"]), "{:?}", c["text"]);
    }
}

#[test]
fn web3forms_detect_golden() {
    let g = golden("web3forms");
    for c in g["detect"].as_array().unwrap() {
        let auth = if c["auth"].is_null() {
            None
        } else {
            Some(&c["auth"])
        };
        let got = web3forms::detect(
            c["from"].as_str().unwrap(),
            auth,
            &strs(&c["sender_domains"]),
            &strs(&c["dkim_domains"]),
        );
        assert_eq!(s(&got.unwrap_or(Value::Null)), s(&c["out"]), "{c}");
    }
    let mut g = g;
    let e = &mut g["extract"][0];
    bless::eq(
        &mut e["schema"],
        web3forms::EXTRACT_SCHEMA.clone(),
        "extract schema",
    );
    bless::eq(
        &mut e["prompt"],
        web3forms::EXTRACT_PROMPT,
        "extract prompt",
    );
    save_keys("web3forms", &g, &["extract"]);
}

fn fields(v: &Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn intake_attribution_utm_config_golden() {
    let g = golden("intake");
    for c in g["utm"].as_array().unwrap() {
        let got: Map<String, Value> = intake::_utm(opt(&c["url"]))
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        assert_eq!(s(&Value::Object(got)), s(&c["out"]), "{:?}", c["url"]);
    }
    for c in g["attribution"].as_array().unwrap() {
        match intake::attribution(&fields(&c["fields"])) {
            Ok(m) => assert_eq!(s(&Value::Object(m)), s(&c["out"]["ok"]), "{}", c["fields"]),
            Err(_) => assert_eq!(c["out"]["err"], "ValueError", "{}", c["fields"]),
        }
    }
    for c in g["with_config"].as_array().unwrap() {
        assert_eq!(
            intake::_with_config(&fields(&c["fields"])).as_deref(),
            c["out"].as_str(),
            "{}",
            c["fields"]
        );
    }
}

#[test]
fn email_intake_pure_golden() {
    let g = golden("email_intake");
    let own: BTreeSet<String> = ["example.com".to_string()].into_iter().collect();
    for c in g["skip_reason"].as_array().unwrap() {
        let h: HashMap<String, String> = c["headers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let got =
            email_intake::skip_reason(&h, c["addr"].as_str().unwrap(), &c["rules"], &own).unwrap();
        assert_eq!(got.as_deref(), c["out"].as_str(), "{c}");
    }
    for c in g["customer_text"].as_array().unwrap() {
        assert_eq!(
            email_intake::customer_text(opt(&c["body"])),
            c["out"].as_str().unwrap(),
            "{:?}",
            c["body"]
        );
    }
    for c in g["split_name"].as_array().unwrap() {
        let (first, last) =
            email_intake::_split_name(opt(&c["name"]), c["email"].as_str().unwrap());
        assert_eq!(first, c["out"][0].as_str().unwrap(), "{c}");
        assert_eq!(last.as_deref(), c["out"][1].as_str(), "{c}");
    }
}

#[test]
fn triage_pure_golden() {
    let g = golden("triage");
    for c in g["parse"].as_array().unwrap() {
        let got = triage::parse(opt(&c["text"]))
            .map(Value::Object)
            .unwrap_or(Value::Null);
        assert_eq!(s(&got), s(&c["out"]), "{:?}", c["text"]);
    }
    for c in g["has_business_signal"].as_array().unwrap() {
        let got = triage::has_business_signal(c["text"].as_str().unwrap(), &strs(&c["signals"]));
        assert_eq!(got.as_deref(), c["out"].as_str(), "{c}");
    }
    for c in g["escalation_rule"].as_array().unwrap() {
        let rule = triage::escalation_rule(c["text"].as_str().unwrap(), &strs(&c["signals"]));
        assert_eq!(rule(opt(&c["answer"])).as_deref(), c["out"].as_str(), "{c}");
    }
    let mut g = g;
    let k = &mut g["consts"][0];
    bless::eq(&mut k["schema"], triage::SCHEMA.clone(), "triage schema");
    bless::eq(
        &mut k["instructions"],
        triage::INSTRUCTIONS,
        "triage instructions",
    );
    save_keys("triage", &g, &["consts"]);
}
