//! Golden replay of crm/dedupe.py `_compare` and `merge_preview`'s field table (fixtures from
//! compat-tests/golden/gen_crm_dedupe.py, recorded from the real Python functions), plus the unit tests of
//! backend/tests/test_crm_ai_unit.py for dedupe.

#[path = "golden/bless.rs"]
mod bless;

use granary_db::rows::crm::{Contact, Organization};
use granary_services::crm::dedupe::{_compare, _label, normalize_name, preview_fields, Rec};
use serde_json::{json, Map, Value};

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../compat-tests/golden/crm_dedupe/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
}

fn base() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!("00000000-0000-0000-0000-000000000001"));
    m.insert("created_at".into(), json!("2026-01-01T00:00:00Z"));
    m.insert("updated_at".into(), json!("2026-01-01T00:00:00Z"));
    m.insert("is_deleted".into(), json!(false));
    m.insert("custom".into(), json!({}));
    m
}

fn org(fields: &Value) -> Rec {
    let mut m = base();
    m.insert("name".into(), json!(""));
    for (k, v) in fields.as_object().unwrap() {
        m.insert(k.clone(), v.clone());
    }
    Rec::Organization(serde_json::from_value::<Organization>(Value::Object(m)).expect("org"))
}

fn contact(fields: &Value) -> Rec {
    let mut m = base();
    m.insert("first_name".into(), json!(""));
    for (k, v) in fields.as_object().unwrap() {
        m.insert(k.clone(), v.clone());
    }
    Rec::Contact(serde_json::from_value::<Contact>(Value::Object(m)).expect("contact"))
}

fn rec(object_type: &str, fields: &Value) -> Rec {
    if object_type == "organization" {
        org(fields)
    } else {
        contact(fields)
    }
}

#[test]
fn golden_compare_and_preview_fields() {
    // The test company is in Japan: a national number (0...) is read with country code 81.
    std::env::set_var("PHONE_DEFAULT_COUNTRY_CODE", "81");
    let mut cases = fixture("compare");
    let mut n = 0;
    for c in cases.as_array_mut().unwrap() {
        let ot = c["object_type"].as_str().unwrap().to_string();
        let (a, b) = (rec(&ot, &c["a"]), rec(&ot, &c["b"]));
        let (score, reasons) = _compare(&ot, &a, &b);
        let (fields, custom_after) = preview_fields(&ot, &a, &b);
        n += 1;
        if bless::blessing() {
            c["score"] = json!(score);
            c["reasons"] = json!(reasons);
            c["label_a"] = json!(_label(&ot, &a));
            c["fields"] = Value::Array(fields);
            c["custom_after"] = serde_json::to_value(&custom_after).unwrap();
            continue;
        }
        assert_eq!(score, c["score"].as_f64().unwrap(), "score {c}");
        let want: Vec<String> = serde_json::from_value(c["reasons"].clone()).unwrap();
        assert_eq!(reasons, want, "reasons {c}");
        assert_eq!(_label(&ot, &a), c["label_a"].as_str().unwrap());
        assert_eq!(Value::Array(fields), c["fields"], "fields {c}");
        assert_eq!(
            serde_json::to_string(&custom_after).unwrap(),
            serde_json::to_string(&c["custom_after"]).unwrap()
        );
    }
    bless::save("crm_dedupe/compare.json", &cases);
    assert!(n >= 2000);
}

// ─── test_crm_ai_unit.py: dedupe ─────────────────────────────────────────────

#[test]
fn test_normalize_name_strips_legal_forms() {
    for (raw, expected) in [
        ("Vandelay Industries Co., Ltd.", "vandelay industries"),
        ("Stark Logistics LLC", "stark logistics"),
        ("Acme Corporation", "acme"),
    ] {
        assert_eq!(normalize_name(Some(raw)), expected);
    }
}

#[test]
fn test_identical_email_scores_very_high() {
    let (score, reasons) = _compare(
        "organization",
        &org(&json!({"name": "Alpha", "email": "a@x.com"})),
        &org(&json!({"name": "Beta", "email": "A@X.com"})),
    );
    assert!(score >= 95.0);
    assert!(reasons.iter().any(|r| r.contains("email")));
}

#[test]
fn test_same_normalized_name_is_flagged() {
    let (score, reasons) = _compare(
        "organization",
        &org(&json!({"name": "Vandelay Industries Ltd."})),
        &org(&json!({"name": "Vandelay Industries Inc"})),
    );
    assert!(score >= 85.0);
    assert!(reasons.iter().any(|r| r.contains("normalization")));
}

#[test]
fn test_unrelated_orgs_score_zero() {
    let (score, _) = _compare(
        "organization",
        &org(&json!({"name": "Alpha Industries"})),
        &org(&json!({"name": "Zulu Trading"})),
    );
    assert_eq!(score, 0.0);
}

#[test]
fn test_same_tax_id_is_flagged() {
    let (score, reasons) = _compare(
        "organization",
        &org(&json!({"name": "Alpha", "tax_id": "TAX-012345678901000"})),
        &org(&json!({"name": "Alpha Corporation", "tax_id": "TAX-012345678901000"})),
    );
    assert!(score >= 95.0);
    assert!(reasons.iter().any(|r| r.contains("tax id")));
}

const ORG1: &str = "11111111-1111-1111-1111-111111111111";
const ORG2: &str = "22222222-2222-2222-2222-222222222222";

#[test]
fn test_same_name_at_different_orgs_is_not_a_duplicate() {
    let (score, _) = _compare(
        "contact",
        &contact(&json!({"first_name": "Bob", "last_name": "Sanders", "organization_id": ORG1})),
        &contact(&json!({"first_name": "Bob", "last_name": "Sanders", "organization_id": ORG2})),
    );
    assert_eq!(score, 0.0);
}

#[test]
fn test_same_name_at_same_org_is_a_duplicate() {
    let (score, _) = _compare(
        "contact",
        &contact(&json!({"first_name": "Bob", "last_name": "Sanders", "organization_id": ORG1})),
        &contact(&json!({"first_name": "Bob", "last_name": "Sanders", "organization_id": ORG1})),
    );
    assert!(score >= 85.0);
}
