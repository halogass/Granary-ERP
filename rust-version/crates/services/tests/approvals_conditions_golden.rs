//! Golden replay: approvals/conditions.rs, engine::facts_fingerprint and notify::email_html against the
//! fixtures first recorded from the retired Python functions (re-record with GOLDEN_BLESS=1 after an intended
//! change and review the diff).
#[path = "golden/bless.rs"]
mod bless;

use granary_services::approvals::conditions::{
    evaluate, failing_preconditions, precondition_meta, rule_meta, validate_condition,
    validate_preconditions, CondError, PRECONDITION_NAMES, RULE_NAMES,
};
use granary_services::approvals::engine::facts_fingerprint;
use serde_json::{json, Value};

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/approvals_conditions/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).expect("fixture JSON")
}

fn outcome<T: serde::Serialize>(r: Result<T, CondError>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"exc": [e.kind, e.msg]}),
    }
}

/// Python equality of JSON results: our `Value`s are compared after a JSON round trip. When blessing, the
/// results are recorded into the golden file `label`.json instead.
fn check(cases: &mut [Value], label: &str, f: impl Fn(&Value) -> Value) {
    let mut failures = vec![];
    for (i, case) in cases.iter_mut().enumerate() {
        let got = f(case);
        if bless::blessing() {
            case["result"] = got;
            continue;
        }
        if got != case["result"] {
            failures.push(format!("#{i}: {case}\n   got {got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{label}: {} of {} differ:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    bless::save(
        &format!("approvals_conditions/{label}.json"),
        &Value::Array(cases.to_vec()),
    );
}

#[test]
fn evaluate_matches_python() {
    let mut cases = load("evaluate");
    check(&mut cases, "evaluate", |c| {
        outcome(evaluate(&c["condition"], &c["snapshot"]).map(|(ok, reasons)| json!([ok, reasons])))
    });
}

#[test]
fn failing_preconditions_matches_python() {
    let mut cases = load("preconditions");
    check(&mut cases, "preconditions", |c| {
        outcome(failing_preconditions(&c["preconditions"], &c["snapshot"]))
    });
}

#[test]
fn validate_matches_python() {
    let mut cases = load("validate");
    check(&mut cases, "validate", |c| {
        let r = if c["fn"] == "condition" {
            validate_condition(&c["input"])
        } else {
            validate_preconditions(&c["input"])
        };
        outcome(r.map(|_| Value::Null))
    });
}

#[test]
fn metadata_matches_python() {
    let mut all = load("meta");
    let meta = &mut all[0];
    // Dict order matters for the API response: compare the serialised text.
    bless::eq(&mut meta["rules"], rule_meta(), "rules");
    bless::eq(
        &mut meta["preconditions"],
        precondition_meta(),
        "preconditions",
    );
    bless::eq(&mut meta["rule_names"], json!(RULE_NAMES), "rule_names");
    bless::eq(
        &mut meta["precondition_names"],
        json!(PRECONDITION_NAMES),
        "precondition_names",
    );
    bless::save("approvals_conditions/meta.json", &Value::Array(all));
}

#[test]
fn facts_fingerprint_matches_python() {
    let mut cases = load("fingerprint");
    check(&mut cases, "fingerprint", |c| {
        let flags: Vec<String> = c["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap().to_string())
            .collect();
        json!(facts_fingerprint(&c["facts"], &flags))
    });
}

#[test]
fn email_html_matches_python() {
    std::env::set_var(
        "JWT_SECRET_KEY",
        "test-jwt-secret-key-at-least-32-characters-long",
    );
    std::env::set_var(
        "APP_SECRET_KEY",
        "test-app-secret-key-at-least-32-characters-long",
    );
    std::env::set_var(
        "SECRET_KEY",
        "test-app-secret-key-at-least-32-characters-long",
    );
    std::env::set_var("POSTGRES_PASSWORD", "granarytest");
    std::env::set_var("SUPERADMIN_PASSWORD", "test-superadmin-password-1234");
    std::env::set_var("MINIO_SECRET_KEY", "test-minio-secret");
    std::env::set_var("WHATSAPP_SERVICE_TOKEN", "test-wa-token");
    std::env::set_var("ENVIRONMENT", "test");
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgresql+asyncpg://granary:granarytest@127.0.0.1:55433/x",
        );
    }
    let mut cases = load("email_html");
    check(&mut cases, "email_html", |c| {
        json!(granary_services::approvals::notify::email_html(
            c["name"].as_str(),
            c["title"].as_str().unwrap(),
            c["body"].as_str().unwrap(),
            c["url"].as_str().unwrap()
        ))
    });
}
