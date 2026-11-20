//! Golden replay of compat-tests/golden/ai_drafting/{scenarios,pure}.json, first recorded from the retired
//! Python drafting service: every call runs on its world (catalog worlds plus contacts and deals) in a
//! transaction rolled back after, and must give exactly the recorded output. Re-record the scenario outputs
//! with GOLDEN_BLESS=1 after an intended change and review the diff.
#[path = "golden/bless.rs"]
mod bless;
mod support;

use granary_compat::pyjson::py_float_repr;
use granary_core::errors::ApiError;
use granary_services::ai::drafting;
use serde_json::{json, Value};
use sqlx::Connection as _;

/// Numbers as Python wrote them: ints stay ints, floats compared by their Python repr.
fn canon(v: &Value) -> Value {
    match v {
        Value::Number(n) => {
            let t = n.to_string();
            if t.contains(['.', 'e', 'E']) {
                json!(format!(
                    "float:{}",
                    py_float_repr(t.parse::<f64>().unwrap())
                ))
            } else {
                json!(format!("int:{t}"))
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), canon(v))).collect()),
        other => other.clone(),
    }
}

fn first_diff(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            if kx != ky {
                return Some(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            x.iter()
                .find_map(|(k, v)| first_diff(&format!("{path}.{k}"), v, &y[k]))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: len {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| first_diff(&format!("{path}[{i}]"), p, q))
        }
        _ if a == b => None,
        _ => Some(format!("{path}: rust {a} != python {b}")),
    }
}

fn err_out(e: ApiError) -> Value {
    match e {
        ApiError::Http { status, detail, .. } => {
            json!({"error": {"status": status.as_u16(), "detail": detail}})
        }
        _ => json!({"error": "500"}),
    }
}

fn normalize(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        if o.get("error") == Some(&json!("500")) {
            o.remove("type");
        }
    }
    v
}

/// The day the outputs were recorded ("today" in them reads as this date).
const RECORDED_ON: &str = "2026-10-15";

#[tokio::test]
async fn ai_drafting_golden_scenarios() {
    const GOLDEN: &str = "ai_drafting/scenarios.json";
    let mut worlds_v = bless::read(GOLDEN);
    let worlds = worlds_v.as_array_mut().unwrap();
    let pool = support::fresh_db("ai_drafting_golden").await;
    let mut conn = pool.acquire().await.unwrap();
    let seeded: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT id, code FROM taxes")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let today = granary_core::clock::today_local(&mut conn)
        .await
        .unwrap()
        .to_string();
    let (mut total, mut bad) = (0, Vec::new());
    for (wi, w) in worlds.iter_mut().enumerate() {
        let mut tx = conn.begin().await.unwrap();
        for stmt in w["sql"].as_array().unwrap() {
            sqlx::raw_sql(stmt.as_str().unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        for (ci, call) in w["calls"].as_array_mut().unwrap().iter_mut().enumerate() {
            total += 1;
            sqlx::raw_sql("SAVEPOINT call")
                .execute(&mut *tx)
                .await
                .unwrap();
            let args = &call["args"].clone();
            let res = match call["fn"].as_str().unwrap() {
                "build_quotation_proposal" => {
                    drafting::build_quotation_proposal(&mut tx, args).await
                }
                "build_invoice_proposal" => drafting::build_invoice_proposal(&mut tx, args).await,
                "build_sop_proposal" => drafting::build_sop_proposal(&mut tx, args).await,
                other => panic!("{other}"),
            };
            let got = match res {
                Ok(v) => json!({"ok": v}),
                Err(e) => err_out(e),
            };
            sqlx::raw_sql("ROLLBACK TO SAVEPOINT call")
                .execute(&mut *tx)
                .await
                .unwrap();
            let mut raw = got.to_string();
            for (id, code) in &seeded {
                raw = raw.replace(&id.to_string(), &format!("SEED-TAX:{code}"));
            }
            if bless::blessing() {
                call["out"] =
                    normalize(serde_json::from_str(&raw.replace(&today, RECORDED_ON)).unwrap());
                continue;
            }
            let got: Value = serde_json::from_str(&raw).unwrap();
            // The outputs carry the day the fixtures were recorded ("today" in the company time zone).
            let want: Value =
                serde_json::from_str(&call["out"].to_string().replace(RECORDED_ON, &today))
                    .unwrap();
            let want = normalize(want);
            if let Some(d) = first_diff("", &canon(&normalize(got)), &canon(&want)) {
                bad.push(format!("world {wi} call {ci} ({}): {d}", call["fn"]));
            }
        }
        tx.rollback().await.unwrap();
    }
    bless::save(GOLDEN, &worlds_v);
    for b in bad.iter().take(40) {
        eprintln!("{b}");
    }
    assert!(
        bad.is_empty(),
        "{} of {total} golden drafting calls differ",
        bad.len()
    );
    eprintln!("{total} golden drafting calls identical");
}

#[test]
fn ai_drafting_golden_pure() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/ai_drafting/pure.json"
    );
    let g: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut n = 0;
    for c in g["num"].as_array().unwrap() {
        let default = c["default"].as_f64().unwrap();
        let got = py_float_repr(drafting::_num(Some(&c["v"]), default));
        assert_eq!(
            got,
            c["out"].as_str().unwrap(),
            "_num({}, {default})",
            c["v"]
        );
        n += 1;
    }
    for c in g["items"].as_array().unwrap() {
        assert_eq!(
            Value::Array(drafting::_parse_items(Some(&c["v"]))),
            c["out"],
            "_parse_items({})",
            c["v"]
        );
        n += 1;
    }
    for c in g["tags"].as_array().unwrap() {
        assert_eq!(
            json!(drafting::_parse_tags(Some(&c["v"]))),
            c["out"],
            "_parse_tags({})",
            c["v"]
        );
        n += 1;
    }
    eprintln!("{n} pure drafting cases identical");
}
