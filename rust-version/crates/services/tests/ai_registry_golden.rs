//! Golden test for ai/registry (pure parts), against fixtures recorded from the real Python functions by
//! compat-tests/golden/gen_ai_registry.py.

mod support;

use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::Connection;

use granary_services::ai::base::{AiError, ChatResult, ProviderError};
use granary_services::ai::registry::{self, seam::Seam, AiConfig};

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/ai_registry/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn cfg_of(v: &Value) -> AiConfig {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect()
}

fn canon(v: &Value) -> String {
    granary_compat::pyjson::dumps(
        &granary_compat::pyjson::PyValue::from(v),
        &Default::default(),
    )
    .unwrap()
}

fn class_of(e: &AiError) -> &'static str {
    match e {
        AiError::Ai { budget: false, .. } => "AIError",
        AiError::Ai { budget: true, .. } => "BudgetExceeded",
        AiError::Value(_) => "ValueError",
        _ => "other",
    }
}

fn check_err(ctx: &str, e: &AiError, want: &Value, problems: &mut Vec<String>) {
    let got = serde_json::json!({"class": class_of(e), "status": e.status_code(), "detail": e.detail(), "str": e.to_string()});
    let mut w = want.clone();
    if w["class"] != "AIError" && w["class"] != "BudgetExceeded" && w["class"] != "ValueError" {
        w["class"] = "other".into();
    }
    if got != w {
        problems.push(format!("{ctx}\n  rust:   {got}\n  python: {w}"));
    }
}

fn finish(name: &str, problems: Vec<String>, total: usize) {
    assert!(
        problems.is_empty(),
        "{name}: {} of {total} differ:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn extract_json_matches() {
    let cases = load("extract_json");
    let mut p = vec![];
    for c in &cases {
        let got = registry::extract_json(c["text"].as_str().unwrap());
        if canon(&got) != canon(&c["out"]) {
            p.push(format!("{:?}: {} != {}", c["text"], got, c["out"]));
        }
    }
    finish("extract_json", p, cases.len());
}

#[test]
fn routing_matches() {
    support::init();
    // ai_ready decrypts keys (email_crypto reads the config, which requires DATABASE_URL); no database is
    // used here, so a placeholder lets this test run alone or first.
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/unused",
        );
    }
    let cases = load("routing");
    let mut p = vec![];
    for c in &cases {
        let cfg = cfg_of(&c["cfg"]);
        let job = c["job"].as_str().unwrap();
        match registry::route_for(&cfg, job) {
            Ok(r) => {
                let got = serde_json::json!({"job": r.job, "provider": r.provider, "model": r.model, "effort": r.effort,
                                             "max_tokens": r.max_tokens, "cache_ttl": r.cache_ttl});
                // Python keeps a non-string effort as is (Route.effort is a str here): compare str(effort).
                let mut want = c.get("route").cloned().unwrap_or(Value::Null);
                if let Some(e) = want.get("effort").filter(|e| e.is_number()).cloned() {
                    want["effort"] = Value::String(e.to_string());
                }
                if want != got {
                    p.push(format!(
                        "{job} {}: {got} != {:?}",
                        c["cfg"],
                        c.get("route").or(c.get("error"))
                    ));
                }
            }
            Err(e) => match c.get("error") {
                Some(w) => check_err(job, &e, w, &mut p),
                None => p.push(format!("{job}: rust error {e}, python {}", c["route"])),
            },
        }
        if registry::ai_ready(&cfg, job) != c["ready"].as_bool().unwrap() {
            p.push(format!("{job}: ai_ready differs"));
        }
    }
    finish("routing", p, cases.len());

    let k = &load("keys")[0];
    let cfg = cfg_of(&k["cfg"]);
    for row in k["keys"].as_array().unwrap() {
        let prov = row["provider"].as_str().unwrap();
        assert_eq!(
            registry::key_for(&cfg, prov)
                .map(Value::from)
                .unwrap_or(Value::Null),
            row["key"],
            "{prov}"
        );
        assert_eq!(
            registry::_base_url_for(&cfg, prov)
                .map(Value::from)
                .unwrap_or(Value::Null),
            row["base_url"],
            "{prov}"
        );
    }
    for row in k["ready"].as_array().unwrap() {
        let job = row["job"].as_str().unwrap();
        assert_eq!(
            registry::ai_ready(&cfg, job),
            row["ready"].as_bool().unwrap(),
            "{job}"
        );
    }
}

#[test]
fn features_match() {
    for c in load("features") {
        assert_eq!(
            registry::feature_enabled(&cfg_of(&c["cfg"]), "scoring"),
            c["out"].as_bool().unwrap(),
            "{}",
            c["cfg"]
        );
    }
}

#[test]
fn prices_match() {
    let cases = load("prices");
    let mut p = vec![];
    for c in &cases {
        let a = c["args"].as_array().unwrap();
        let n = |i: usize| a[i].as_i64().unwrap();
        let got = registry::estimate_cost(
            &cfg_of(&c["cfg"]),
            c["model"].as_str().unwrap(),
            n(0),
            n(1),
            n(2),
            n(3),
            n(4),
            a[5].as_bool().unwrap(),
        );
        match (got, c.get("error")) {
            (Ok(v), None) => {
                let got = v.map(|d| granary_compat::pyjson::str_decimal(&d.into()));
                if got.as_deref() != c["out"].as_str() {
                    p.push(format!("{c}: {got:?}"));
                }
            }
            (Err(e), Some(w)) => check_err("price", &e, w, &mut p),
            (g, _) => p.push(format!("{c}: {g:?}")),
        }
    }
    finish("prices", p, cases.len());
}

#[test]
fn friendly_errors_match() {
    let cases = load("friendly");
    for c in &cases {
        let model = c["model"].as_str().unwrap();
        let e = if let Some(d) = c.get("http") {
            AiError::ai(d.as_str().unwrap())
        } else {
            let kind = c["kind"].as_str().unwrap();
            // Python: `getattr(e, "message", None) or str(e)`.
            let message = c["message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| c["str"].as_str().unwrap().into());
            if kind == "ValueError" {
                AiError::Value(message)
            } else {
                AiError::Provider(Box::new(ProviderError {
                    status: c["status"].as_u64().map(|s| s as u16),
                    kind: kind.into(),
                    message,
                    body: c.get("body").filter(|b| !b.is_null()).cloned(),
                    display: None,
                }))
            }
        };
        assert_eq!(
            registry::friendly_error(&e, model),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
    for c in load("refusals") {
        let r = ChatResult {
            stop_reason: "refusal".into(),
            refusal_category: c["category"].as_str().map(str::to_string),
            ..Default::default()
        };
        assert_eq!(registry::refusal_message(&r), c["out"].as_str().unwrap());
    }
    for c in load("money") {
        let d: Decimal = c["d"].as_str().unwrap().parse().unwrap();
        assert_eq!(registry::money(d), c["out"].as_str().unwrap());
    }
}

#[tokio::test]
async fn budget_gate_matches() {
    support::init();
    let mut conn = sqlx::PgConnection::connect(&support::admin_url())
        .await
        .unwrap();
    let cases = load("budget");
    let mut p = vec![];
    let seam = Seam::install().await;
    for c in &cases {
        let spent: Decimal = c["spent"].as_str().unwrap().parse().unwrap();
        let per_job: Vec<(String, Decimal)> = c["per_job"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().parse().unwrap()))
            .collect();
        seam.month_spend(move |job| {
            Ok(match job {
                Some(j) => per_job
                    .iter()
                    .find(|(k, _)| k == j)
                    .map(|(_, v)| *v)
                    .unwrap_or_default(),
                None => spent,
            })
        });
        let cfg = cfg_of(&c["cfg"]);
        let job = c["job"].as_str().unwrap();
        match registry::check_job_budget(&mut conn, &cfg, job).await {
            Ok(st) => {
                if Some(st.level()) != c["level"].as_str() {
                    p.push(format!(
                        "{job} {}: level {} vs {}",
                        c["spent"],
                        st.level(),
                        c["level"]
                    ));
                }
            }
            Err(e) => match c.get("error") {
                Some(w) => check_err(&format!("{job} {} {}", c["spent"], c["cfg"]), &e, w, &mut p),
                None => p.push(format!("{job}: rust error {e}")),
            },
        }
        match registry::check_budget(&mut conn, &cfg, job).await {
            Ok(r) => {
                if c.get("check_budget").map(|v| v.as_str()) != Some(r.as_deref()) {
                    p.push(format!(
                        "check_budget {job}: {r:?} vs {:?}",
                        c.get("check_budget")
                    ));
                }
            }
            Err(e) => match c.get("check_budget_error") {
                Some(w) => check_err("check_budget", &e, w, &mut p),
                None => p.push(format!("check_budget {job}: rust error {e}")),
            },
        }
    }
    drop(seam);
    finish("budget", p, cases.len());
}
