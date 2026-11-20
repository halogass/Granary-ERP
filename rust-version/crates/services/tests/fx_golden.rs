//! Golden cases for fx's pure parts, first recorded from the retired Python (settings parsers re-recorded with
//! GOLDEN_BLESS=1: a missing setting now falls back to USD).
#[path = "golden/bless.rs"]
mod bless;

use std::str::FromStr;

use granary_services::fx::{
    get_active_currencies, get_functional_currency, py_div_quantize, py_json_number_decimal,
};
use rust_decimal::Decimal;
use serde_json::Value;

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/fx/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).unwrap()
}

/// Python's str(Decimal) may use exponent notation ("2E-12"): compare value and scale.
fn same(rust: Decimal, py: &str) -> bool {
    let e = Decimal::from_str(py)
        .or_else(|_| Decimal::from_scientific(py))
        .unwrap_or_else(|_| panic!("unparseable {py}"));
    rust == e && rust.scale() == e.scale()
}

#[test]
fn golden_division() {
    let cases = load("division");
    let mut checked = 0;
    for c in &cases {
        let a = c["a"].as_str().unwrap();
        let b = c["b"].as_str().unwrap();
        let (Ok(a), Ok(b)) = (Decimal::from_str(a), Decimal::from_str(b)) else {
            continue;
        };
        let got = py_div_quantize(a, b, 12);
        match c.get("out") {
            Some(out) => {
                let out = out.as_str().unwrap();
                let got = got.unwrap_or_else(|e| panic!("{a}/{b}: {e}"));
                assert!(same(got, out), "{a}/{b}: rust {got} python {out}");
            }
            None => assert!(got.is_err(), "{a}/{b} should fail"),
        }
        checked += 1;
    }
    assert!(checked > 600, "{checked}");
}

#[test]
fn golden_json_numbers() {
    let cases = load("json_numbers");
    let mut checked = 0;
    for c in &cases {
        let lit = c["lit"].as_str().unwrap();
        let out = c["out"].as_str().unwrap();
        let v: Value = serde_json::from_str(lit).unwrap();
        // Python values outside rust_decimal's range (1e308, 5e-324) cannot be represented at all.
        let Ok(exp) = Decimal::from_str(out).or_else(|_| Decimal::from_scientific(out)) else {
            continue;
        };
        let got = py_json_number_decimal(&v).unwrap_or_else(|| panic!("{lit}"));
        assert_eq!(got, exp, "{lit}");
        assert_eq!(got.scale(), exp.scale(), "{lit}");
        checked += 1;
    }
    assert!(checked > 600, "{checked}");
}

#[test]
fn golden_settings_parsers() {
    let mut cases = load("settings_parsers");
    for c in cases.iter_mut() {
        let value = c["value"].as_str().map(|s| s.to_string());
        let f = c["fn"].as_str().unwrap();
        match f {
            "active" | "active_missing" => {
                let rows: Vec<(String, Option<String>)> = if f == "active" {
                    vec![("currency.active".into(), value.clone())]
                } else {
                    vec![]
                };
                let got = get_active_currencies(&rows);
                if bless::blessing() {
                    match &got {
                        Ok(v) => c["out"] = serde_json::json!(v),
                        Err(_) => {
                            c.as_object_mut().unwrap().shift_remove("out");
                        }
                    }
                }
                match c.get("out") {
                    Some(out) => {
                        let out: Vec<String> = serde_json::from_value(out.clone()).unwrap();
                        assert_eq!(got.unwrap(), out, "{value:?}");
                    }
                    None => assert!(got.is_err(), "{value:?}"),
                }
            }
            _ => {
                let rows: Vec<(String, Option<String>)> = if f == "functional" {
                    vec![("currency.functional".into(), value.clone())]
                } else {
                    vec![]
                };
                bless::eq(
                    &mut c["out"],
                    get_functional_currency(&rows),
                    format!("{value:?}"),
                );
            }
        }
    }
    bless::save("fx/settings_parsers.json", &serde_json::Value::Array(cases));
}
