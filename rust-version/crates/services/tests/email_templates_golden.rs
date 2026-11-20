//! Golden replay: email_templates.rs against the HTML recorded from the real Python templates
//! (compat-tests/golden/gen_email_templates.py). The outputs re-record with `GOLDEN_BLESS=1`
//! (tests/golden/bless.rs): the Rust templates are now the reference.

#[path = "golden/bless.rs"]
mod bless;

use granary_services::email_templates as t;
use serde_json::Value;

fn load(dir: &str, name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/{dir}/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).expect("fixture JSON")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn o(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn every_template_is_byte_identical_to_python() {
    let mut cases = load("email_templates", "templates");
    assert!(cases.len() >= 300);
    let mut failures = 0;
    for (i, slot) in cases.iter_mut().enumerate() {
        let c = &slot.clone();
        let a = c["args"].as_array().unwrap();
        let got = match c["fn"].as_str().unwrap() {
            "test_email_template" => t::test_email_template(s(&a[0])),
            "quotation_email_template" => t::quotation_email_template(
                s(&a[0]),
                s(&a[1]),
                s(&a[2]),
                s(&a[3]),
                s(&a[4]),
                s(&a[5]),
                s(&a[6]),
            ),
            "invoice_email_template" => t::invoice_email_template(
                s(&a[0]),
                s(&a[1]),
                s(&a[2]),
                s(&a[3]),
                s(&a[4]),
                s(&a[5]),
                s(&a[6]),
                s(&a[7]),
            ),
            "notification_email_template" => t::notification_email_template(
                s(&a[0]),
                s(&a[1]),
                s(&a[2]),
                o(&a[3]),
                o(&a[4]),
                s(&a[5]),
                s(&a[6]),
            ),
            "portal_invite_email_template" => t::portal_invite_email_template(
                s(&a[0]),
                s(&a[1]),
                s(&a[2]),
                s(&a[3]),
                s(&a[4]),
                s(&a[5]),
                s(&a[6]),
            ),
            "password_reset_email_template" => t::password_reset_email_template(
                s(&a[0]),
                s(&a[1]),
                a[2].as_i64().unwrap(),
                s(&a[3]),
                s(&a[4]),
            ),
            "document_email_template" => t::document_email_template(
                s(&a[0]),
                s(&a[1]),
                s(&a[2]),
                s(&a[3]),
                s(&a[4]),
                s(&a[5]),
                s(&a[6]),
                s(&a[7]),
            ),
            other => panic!("unknown fn {other}"),
        };
        if bless::blessing() {
            slot["out"] = Value::String(got);
            continue;
        }
        let want = c["out"].as_str().unwrap();
        if got != want {
            failures += 1;
            if failures <= 3 {
                let at = got
                    .chars()
                    .zip(want.chars())
                    .take_while(|(x, y)| x == y)
                    .count();
                let ctx: String = want.chars().skip(at.saturating_sub(40)).take(120).collect();
                let gctx: String = got.chars().skip(at.saturating_sub(40)).take(120).collect();
                eprintln!(
                    "case {i} {}: differs at char {at}\n want: {ctx:?}\n  got: {gctx:?}",
                    c["fn"]
                );
            }
        }
    }
    assert_eq!(failures, 0, "{failures} of {} cases differ", cases.len());
    bless::save("email_templates/templates.json", &Value::Array(cases));
}

#[test]
fn defaults_match_python() {
    // test_email_template() with its default from_name equals the explicit call.
    assert_eq!(
        t::test_email_template(t::DEFAULT_FROM_NAME),
        t::test_email_template("Granary")
    );
}
