//! Golden tests for granary_compat::pyfmt against fixtures recorded from CPython 3.12
//! (compat-tests/golden/gen_pyjson.py). Every case must match byte for byte.

use chrono::{NaiveDate, NaiveDateTime};
use granary_compat::pyfmt::*;
use granary_compat::pyjson::{PyDateTime, PyDecimal, PyTz};
use serde_json::Value;

fn load(name: &str) -> Vec<Value> {
    let p = format!(
        "{}/../../compat-tests/golden/pyfmt/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn report(name: &str, total: usize, failures: Vec<String>) {
    if !failures.is_empty() {
        let shown: Vec<_> = failures.iter().take(25).cloned().collect();
        panic!(
            "{name}: {}/{total} cases differ:\n{}",
            failures.len(),
            shown.join("\n")
        );
    }
    assert!(total > 0, "{name}: no cases");
}

fn num(v: &Value) -> PyNum {
    let a = v.as_array().unwrap();
    let s = a[1].as_str().unwrap();
    match a[0].as_str().unwrap() {
        "int" => PyNum::Int(s.parse().unwrap()),
        "float" => PyNum::Float(f64::from_bits(u64::from_str_radix(s, 16).unwrap())),
        "decimal" => PyNum::Decimal(PyDecimal::parse(s).unwrap()),
        "decimal_rd" => PyNum::from(s.parse::<rust_decimal::Decimal>().unwrap()),
        t => panic!("not a number tag {t}"),
    }
}

enum When {
    Dt(PyDateTime),
    Date(NaiveDate),
}

fn when(v: &Value) -> When {
    let a = v.as_array().unwrap();
    let s = a[1].as_str().unwrap();
    let parse = |s: &str| {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
            .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
            .unwrap()
    };
    match a[0].as_str().unwrap() {
        "date" => When::Date(NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()),
        "datetime" => When::Dt(PyDateTime {
            local: parse(s),
            tz: match &a[2] {
                Value::Null => None,
                Value::String(u) if u == "utc" => Some(PyTz::Utc),
                Value::Array(f) => Some(PyTz::Fixed(f[1].as_i64().unwrap() as i32)),
                o => panic!("bad tz {o}"),
            },
        }),
        "datetime_zone" => When::Dt(PyDateTime::from_utc_in_zone(
            parse(s),
            a[2].as_str().unwrap().parse().unwrap(),
        )),
        "datetime_wall" => When::Dt(PyDateTime::in_zone(
            parse(s),
            a[2].as_str().unwrap().parse().unwrap(),
            a[3].as_u64().unwrap() as u8,
        )),
        t => panic!("not a datetime tag {t}"),
    }
}

#[test]
fn golden_format() {
    let cases = load("format.json");
    let mut fails = vec![];
    for c in &cases {
        let spec = c["spec"].as_str().unwrap();
        let got = format_number(&num(&c["value"]), spec);
        let exp = &c["expected"];
        let ok = match (&got, exp.get("ok")) {
            (Ok(g), Some(e)) => g == e.as_str().unwrap(),
            (Err(_), None) => true,
            _ => false,
        };
        if !ok {
            fails.push(format!(
                "value={} spec={spec:?} expected={exp} got={got:?}",
                c["value"]
            ));
        }
    }
    report("format", cases.len(), fails);
}

#[test]
fn golden_strftime() {
    let cases = load("strftime.json");
    let mut fails = vec![];
    for c in &cases {
        let fmt = c["fmt"].as_str().unwrap();
        let got = match when(&c["value"]) {
            When::Dt(d) => strftime(&d, fmt),
            When::Date(d) => strftime_date(d, fmt),
        };
        if got != c["expected"].as_str().unwrap() {
            fails.push(format!(
                "value={} fmt={fmt:?} expected={} got={got:?}",
                c["value"], c["expected"]
            ));
        }
    }
    report("strftime", cases.len(), fails);
}

#[test]
fn golden_html_escape() {
    let cases = load("html_escape.json");
    let mut fails = vec![];
    for c in &cases {
        let got = html_escape(c["s"].as_str().unwrap(), c["quote"].as_bool().unwrap());
        if got != c["expected"].as_str().unwrap() {
            fails.push(format!(
                "s={} quote={} expected={} got={got:?}",
                c["s"], c["quote"], c["expected"]
            ));
        }
    }
    report("html_escape", cases.len(), fails);
}
