//! Golden fixtures recorded from backend/app/services/wa_signature.py
//! (compat-tests/golden/gen_wa_signature.py).

use granary_services::wa_signature::{
    display_name_of, message_key, short_name, sign, strip_signature,
};
use serde_json::Value;

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/wa_signature/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn golden_short_name() {
    let cases = load("short_name");
    for c in &cases {
        assert_eq!(
            short_name(opt(&c["in"])),
            c["out"].as_str().unwrap(),
            "{:?}",
            c["in"]
        );
    }
    assert!(cases.len() > 100);
}

#[test]
fn golden_display_name() {
    for c in load("display_name") {
        let u = &c["user"];
        let got = display_name_of(
            opt(&u["wa_signature_name"]),
            opt(&u["name"]),
            u["email"].as_str().unwrap(),
        );
        assert_eq!(got, c["out"].as_str().unwrap(), "{u:?}");
    }
}

#[test]
fn golden_sign() {
    for c in load("sign") {
        assert_eq!(
            sign(opt(&c["text"]), c["sig"].as_str().unwrap()),
            c["out"].as_str().unwrap(),
            "{c:?}"
        );
    }
}

#[test]
fn golden_strip_signature() {
    for c in load("strip_signature") {
        assert_eq!(
            strip_signature(opt(&c["in"])),
            c["out"].as_str().unwrap(),
            "{:?}",
            c["in"]
        );
    }
}

#[test]
fn golden_message_key() {
    for c in load("message_key") {
        assert_eq!(
            message_key(opt(&c["in"])).as_deref(),
            opt(&c["out"]),
            "{:?}",
            c["in"]
        );
    }
}
