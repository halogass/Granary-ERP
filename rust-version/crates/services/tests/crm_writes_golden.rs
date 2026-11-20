//! Golden replay of crm/writes.py (fixtures from compat-tests/golden/gen_crm_writes.py, recorded from the
//! real Python functions) plus the pure unit tests of backend/tests/test_crm_ai_unit.py for writes.

use granary_services::crm::writes::{
    check_refs, py_date_fromisoformat, py_datetime_fromisoformat, resolve_refs, validate_change,
    WriteError,
};
use serde_json::{json, Map, Value};

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../compat-tests/golden/crm_writes/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
}

fn as_maps(v: &Value) -> Vec<Map<String, Value>> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_object().unwrap().clone())
        .collect()
}

fn out_of<T: Into<Value>>(r: Result<T, WriteError>) -> Value {
    match r {
        Ok(v) => json!({"ok": v.into()}),
        Err(WriteError::Invalid(m)) => json!({"err": m}),
        Err(WriteError::Api(e)) => panic!("unexpected {e}"),
    }
}

#[test]
fn golden_validate_change() {
    let cases = fixture("validate_change");
    for c in cases.as_array().unwrap() {
        let got = out_of(
            validate_change(
                c["object_type"].as_str().unwrap(),
                c["action"].as_str().unwrap(),
                &c["fields"],
                c["allow_refs"].as_bool().unwrap(),
            )
            .map(Value::Object),
        );
        assert_eq!(got, c["out"], "case {c}");
    }
}

#[test]
fn golden_isoformat() {
    for c in fixture("isoformat").as_array().unwrap() {
        let s = c["s"].as_str().unwrap();
        let d = py_date_fromisoformat(s).map(|d| d.to_string());
        assert_eq!(json!(d), c["date"], "date {s:?}");
        let dt = py_datetime_fromisoformat(s);
        let shown = dt.map(|(n, _)| {
            let base = n.format("%Y-%m-%dT%H:%M:%S").to_string();
            let us = n.and_utc().timestamp_subsec_micros();
            if us != 0 {
                format!("{base}.{us:06}")
            } else {
                base
            }
        });
        assert_eq!(json!(shown), c["datetime"], "datetime {s:?}");
        if c["datetime"].is_string() {
            let off = dt.unwrap().1.map(|(s, us)| s * 1_000_000 + us);
            assert_eq!(json!(off), c["offset_us"], "offset {s:?}");
        }
    }
}

#[test]
fn golden_refs() {
    for c in fixture("refs").as_array().unwrap() {
        let earlier = as_maps(&c["earlier"]);
        let mut prop = c["prop"].as_object().unwrap().clone();
        let got = check_refs(&mut prop, &earlier);
        assert_eq!(json!(got), c["check"]["ok"], "check {c}");
        assert_eq!(Value::Object(prop), c["check"]["prop"], "prop {c}");
        let after = c["prop"]["after"].as_object().unwrap().clone();
        assert_eq!(
            out_of(resolve_refs(&after, &earlier).map(Value::Object)),
            c["resolve"],
            "resolve {c}"
        );
    }
}

// ─── test_crm_ai_unit.py: writes ─────────────────────────────────────────────

fn err(r: Result<Map<String, Value>, WriteError>) -> String {
    match r {
        Err(WriteError::Invalid(m)) => m,
        other => panic!("expected a WriteError, got {other:?}"),
    }
}

#[test]
fn test_validate_change_accepts_known_fields() {
    let out = validate_change(
        "deal",
        "update",
        &json!({"stage": "proposal", "probability": 70}),
        false,
    )
    .unwrap();
    assert_eq!(
        Value::Object(out),
        json!({"stage": "proposal", "probability": 70})
    );
}

#[test]
fn test_validate_change_rejects_unknown_field() {
    assert!(err(validate_change(
        "deal",
        "update",
        &json!({"nonexistent": 1}),
        false
    ))
    .contains("not writable"));
}

#[test]
fn test_validate_change_rejects_identity_and_system_fields() {
    for field in ["created_by_id", "is_deleted", "id", "deal_number"] {
        err(validate_change(
            "deal",
            "update",
            &json!({field: "x"}),
            false,
        ));
    }
}

#[test]
fn test_validate_change_rejects_unknown_object_type() {
    assert!(err(validate_change(
        "invoice",
        "update",
        &json!({"title": "x"}),
        false
    ))
    .contains("object_type"));
}

#[test]
fn test_validate_change_rejects_unknown_action() {
    assert!(err(validate_change(
        "deal",
        "delete",
        &json!({"title": "x"}),
        false
    ))
    .contains("create or update"));
}

#[test]
fn test_validate_change_requires_fields_on_create() {
    assert!(err(validate_change(
        "organization",
        "create",
        &json!({"city": "Osaka"}),
        false
    ))
    .contains("Missing required"));
    assert_eq!(
        validate_change(
            "organization",
            "create",
            &json!({"name": "Test Ltd"}),
            false
        )
        .unwrap()["name"],
        "Test Ltd"
    );
}

#[test]
fn test_validate_change_allows_custom_passthrough() {
    let out = validate_change(
        "organization",
        "update",
        &json!({"custom": {"icp_tier": "Tier 1"}}),
        false,
    )
    .unwrap();
    assert_eq!(out["custom"], json!({"icp_tier": "Tier 1"}));
}

#[test]
fn test_activity_requires_its_polymorphic_target() {
    assert!(err(validate_change(
        "activity",
        "create",
        &json!({"subject": "Call"}),
        false
    ))
    .contains("Missing required"));
}
