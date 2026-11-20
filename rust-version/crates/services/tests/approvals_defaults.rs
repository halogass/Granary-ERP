//! approvals/defaults.rs: POLICIES dump as recorded in the golden (key order, value types, text; re-record with
//! GOLDEN_BLESS=1 after an intended change) and match what the seed inserts (crates/db/src/seed/seed_data.json).
#[path = "golden/bless.rs"]
mod bless;

use granary_compat::pyjson::{self, DumpOpts, PyValue};
use granary_services::approvals::defaults::{DISCOUNT_ALLOWANCES, OWNER_SLA_HOURS, POLICIES};
use serde_json::Value;

fn golden() -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/approvals_adapters/cases.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn policies_dump_like_python() {
    let mut g = golden();
    let ours = pyjson::dumps(
        &PyValue::List(POLICIES.iter().map(PyValue::from).collect()),
        &DumpOpts::default(),
    )
    .unwrap();
    bless::eq(&mut g["policies_dump"], ours, "policies_dump");
    let allowances = pyjson::dumps(
        &PyValue::List(DISCOUNT_ALLOWANCES.iter().map(PyValue::from).collect()),
        &DumpOpts::default(),
    )
    .unwrap();
    bless::eq(
        &mut g["discount_allowances_dump"],
        allowances,
        "discount_allowances_dump",
    );
    bless::update("approvals_adapters/cases.json", |v| {
        v["policies_dump"] = g["policies_dump"].clone();
        v["discount_allowances_dump"] = g["discount_allowances_dump"].clone();
    });
    assert_eq!(OWNER_SLA_HOURS, 24);
}

#[test]
fn policies_match_the_seed() {
    let seed: Value =
        serde_json::from_str(include_str!("../../db/src/seed/seed_data.json")).unwrap();
    let seeded = seed["approval_policies"].as_array().unwrap();
    assert_eq!(seeded.len(), POLICIES.len());
    for (spec, policy) in seeded.iter().zip(POLICIES.iter()) {
        let mut p = policy.as_object().unwrap().clone();
        let steps = p.remove("steps").unwrap();
        assert_eq!(
            &spec["policy"],
            &Value::Object(p),
            "{}",
            policy["entity_type"]
        );
        assert_eq!(&spec["steps"], &steps, "{}", policy["entity_type"]);
    }
}

/// test_approval_conditions.py::test_signing_and_login_settings_follow_the_sops
#[test]
fn test_signing_and_login_settings_follow_the_sops() {
    let by_type = |t: &str| {
        POLICIES
            .iter()
            .find(|p| p["entity_type"] == t)
            .unwrap()
            .clone()
    };
    for counterparty in ["sales_contract", "offer_letter"] {
        assert_eq!(by_type(counterparty)["signature_flow"], "counterparty");
    }
    // The approval is our signature: these carry the approval stamp + QR and are never e-signed internally.
    for qr_only in ["quotation", "invoice", "purchase_order", "work_order"] {
        let p = by_type(qr_only);
        assert_eq!(
            p.get("signature_flow")
                .and_then(Value::as_str)
                .unwrap_or("none"),
            "none"
        );
    }
    assert_eq!(by_type("offer_letter")["requires_login"], Value::Bool(true));
    // not by_type["payroll_run"].get("qr_enabled")
    assert!(!by_type("payroll_run")
        .get("qr_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false));
}
