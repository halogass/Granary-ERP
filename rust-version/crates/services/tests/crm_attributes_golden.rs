//! services/crm/attributes.py: golden cases recorded from the real coerce_value / validate_key, plus the
//! attribute tests of backend/tests/test_crm_ai_unit.py.
mod crm_s15_common;

use granary_services::crm::attributes::{coerce_value, validate_key, AttrDef, AttributeFail};
use serde_json::{json, Value};

/// test_crm_ai_unit.py `_Def`.
struct Def {
    field_type: String,
    options: Option<Value>,
    label: String,
}

impl AttrDef for Def {
    fn field_type(&self) -> &str {
        &self.field_type
    }
    fn options(&self) -> Option<&Value> {
        self.options.as_ref()
    }
    fn label(&self) -> &str {
        &self.label
    }
}

fn d(ft: &str, options: Value, label: &str) -> Def {
    Def {
        field_type: ft.into(),
        options: if options.is_null() {
            None
        } else {
            Some(options)
        },
        label: label.into(),
    }
}

fn attr_msg(r: Result<Option<Value>, AttributeFail>) -> String {
    match r {
        Err(AttributeFail::Attr(e)) => e.0,
        other => panic!("expected AttributeError_, got {other:?}"),
    }
}

#[test]
fn coerce_value_golden() {
    let cases = crm_s15_common::load("crm_attributes/coerce.json");
    let mut bad = Vec::new();
    for c in cases.as_array().unwrap() {
        let def = &c["def"];
        let dd = d(
            def[0].as_str().unwrap(),
            def[1].clone(),
            def[2].as_str().unwrap(),
        );
        let got = coerce_value(&dd, &c["value"]);
        let ok = match (
            &got,
            c.get("ok"),
            c.get("err"),
            c.get("exc"),
            c.get("nonfinite"),
        ) {
            (Ok(v), Some(exp), ..) => v.clone().unwrap_or(Value::Null) == *exp,
            (Err(AttributeFail::Attr(e)), _, Some(exp), ..) => e.0 == exp.as_str().unwrap(),
            (Err(AttributeFail::Api(_)), _, _, Some(_), _) => true,
            (Err(AttributeFail::Api(_)), _, _, _, Some(_)) => true,
            _ => false,
        };
        if !ok {
            bad.push(format!("{c} -> {got:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad[..bad.len().min(30)].join("\n")
    );
}

#[test]
fn validate_key_golden() {
    for c in crm_s15_common::load("crm_attributes/keys.json")
        .as_array()
        .unwrap()
    {
        let got = validate_key(c["key"].as_str().unwrap());
        match (got, c.get("ok")) {
            (Ok(k), Some(exp)) => assert_eq!(k, exp.as_str().unwrap()),
            (Err(e), None) => assert_eq!(e.0, c["err"].as_str().unwrap()),
            (g, _) => panic!("{c}: {g:?}"),
        }
    }
}

// ─── backend/tests/test_crm_ai_unit.py (attribute value coercion) ─────────────

#[test]
fn test_select_rejects_value_outside_choices() {
    let def = d(
        "select",
        json!({"choices": ["Tier 1", "Tier 2"]}),
        "ICP Tier",
    );
    assert_eq!(
        coerce_value(&def, &json!("Tier 1")).unwrap(),
        Some(json!("Tier 1"))
    );
    assert!(attr_msg(coerce_value(&def, &json!("Tier 9"))).contains("must be one of"));
}

#[test]
fn test_multi_select_rejects_any_bad_member() {
    let def = d(
        "multi_select",
        json!({"choices": ["Kanto", "Okinawa"]}),
        "Regions",
    );
    assert_eq!(
        coerce_value(&def, &json!(["Kanto"])).unwrap(),
        Some(json!(["Kanto"]))
    );
    assert!(attr_msg(coerce_value(&def, &json!(["Kanto", "Atlantis"]))).contains("invalid"));
}

#[test]
fn test_number_and_rating_bounds() {
    assert_eq!(
        coerce_value(&d("number", json!({}), "Field"), &json!("12.5")).unwrap(),
        Some(json!(12.5))
    );
    assert!(attr_msg(coerce_value(
        &d("number", json!({}), "Field"),
        &json!("abc")
    ))
    .contains("must be a number"));
    assert!(attr_msg(coerce_value(
        &d("rating", json!({"max": 5}), "Field"),
        &json!(9)
    ))
    .contains("between 0 and 5"));
}

#[test]
fn test_email_and_url_validation() {
    assert_eq!(
        coerce_value(&d("email", json!({}), "Field"), &json!("a@b.co")).unwrap(),
        Some(json!("a@b.co"))
    );
    attr_msg(coerce_value(
        &d("email", json!({}), "Field"),
        &json!("not-an-email"),
    ));
    assert!(attr_msg(coerce_value(
        &d("url", json!({}), "Field"),
        &json!("example.com")
    ))
    .contains("http"));
}

#[test]
fn test_empty_value_clears() {
    assert_eq!(
        coerce_value(&d("text", json!({}), "Field"), &json!("")).unwrap(),
        None
    );
    assert_eq!(
        coerce_value(&d("text", json!({}), "Field"), &Value::Null).unwrap(),
        None
    );
}
