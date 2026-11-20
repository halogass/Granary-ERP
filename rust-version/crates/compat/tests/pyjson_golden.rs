//! Golden tests for granary_compat::pyjson against fixtures recorded from CPython 3.12
//! (compat-tests/golden/gen_pyjson.py). Every case must match byte for byte.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use granary_compat::pyjson::*;
use serde_json::Value;

fn load(name: &str) -> Vec<Value> {
    let p = format!(
        "{}/../../compat-tests/golden/pyjson/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn tz(v: &Value) -> Option<PyTz> {
    match v {
        Value::Null => None,
        Value::String(s) if s == "utc" => Some(PyTz::Utc),
        Value::Array(a) => Some(PyTz::Fixed(a[1].as_i64().unwrap() as i32)),
        other => panic!("bad tz {other}"),
    }
}

/// Decode the typed value encoding documented in gen_pyjson.py.
pub fn decode(v: &Value) -> PyValue {
    let a = v.as_array().expect("typed value");
    let tag = a[0].as_str().unwrap();
    let s = || a[1].as_str().unwrap();
    match tag {
        "none" => PyValue::None,
        "bool" => PyValue::Bool(a[1].as_bool().unwrap()),
        "int" => PyValue::Int(s().parse().unwrap()),
        "float" => PyValue::Float(f64::from_bits(u64::from_str_radix(s(), 16).unwrap())),
        "str" => PyValue::Str(s().to_string()),
        "list" => PyValue::List(a[1].as_array().unwrap().iter().map(decode).collect()),
        "tuple" => PyValue::Tuple(a[1].as_array().unwrap().iter().map(decode).collect()),
        "dict" => PyValue::Dict(
            a[1].as_array()
                .unwrap()
                .iter()
                .map(|kv| (kv[0].as_str().unwrap().to_string(), decode(&kv[1])))
                .collect(),
        ),
        "decimal" => PyValue::Decimal(PyDecimal::parse(s()).unwrap()),
        "decimal_rd" => {
            let d: rust_decimal::Decimal = s().parse().unwrap();
            PyValue::Decimal(PyDecimal::from(d))
        }
        "datetime" => PyValue::DateTime(PyDateTime {
            local: NaiveDateTime::parse_from_str(s(), "%Y-%m-%dT%H:%M:%S%.f").unwrap(),
            tz: tz(&a[2]),
        }),
        "datetime_zone" => {
            let utc = NaiveDateTime::parse_from_str(s(), "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(s(), "%Y-%m-%dT%H:%M:%S"))
                .unwrap();
            let zone: chrono_tz::Tz = a[2].as_str().unwrap().parse().unwrap();
            PyValue::DateTime(PyDateTime::from_utc_in_zone(utc, zone))
        }
        "datetime_wall" => PyValue::DateTime(PyDateTime::in_zone(
            NaiveDateTime::parse_from_str(s(), "%Y-%m-%dT%H:%M:%S%.f").unwrap(),
            a[2].as_str().unwrap().parse().unwrap(),
            a[3].as_u64().unwrap() as u8,
        )),
        "date" => PyValue::Date(NaiveDate::parse_from_str(s(), "%Y-%m-%d").unwrap()),
        "time" => PyValue::Time(PyTime {
            time: NaiveTime::parse_from_str(s(), "%H:%M:%S%.f").unwrap(),
            offset: match tz(&a[2]) {
                None => None,
                Some(PyTz::Utc) => Some(0),
                Some(PyTz::Fixed(o)) => Some(o),
                Some(PyTz::Zone(_)) | Some(PyTz::ZoneFold1(_)) => unreachable!(),
            },
        }),
        "uuid" => PyValue::Uuid(s().parse().unwrap()),
        other => panic!("unknown tag {other}"),
    }
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

#[test]
fn golden_dumps() {
    let cases = load("dumps.json");
    let mut fails = vec![];
    for c in &cases {
        let o = &c["opts"];
        let opts = DumpOpts {
            sort_keys: o["sort_keys"].as_bool().unwrap(),
            ensure_ascii: o["ensure_ascii"].as_bool().unwrap(),
            separators: o["separators"].as_array().map(|s| {
                (
                    s[0].as_str().unwrap().to_string(),
                    s[1].as_str().unwrap().to_string(),
                )
            }),
            indent: o["indent"].as_u64().map(|n| n as usize),
            default_str: o["default_str"].as_bool().unwrap(),
            allow_nan: o["allow_nan"].as_bool().unwrap(),
        };
        let got = dumps(&decode(&c["value"]), &opts);
        let exp = &c["expected"];
        let ok = match (
            &got,
            exp.get("ok"),
            exp.get("error").and_then(Value::as_str),
        ) {
            (Ok(g), Some(e), _) => g == e.as_str().unwrap(),
            (Err(PyError::TypeError(_)), _, Some("TypeError")) => true,
            (Err(PyError::ValueError(_)), _, Some("ValueError")) => true,
            _ => false,
        };
        if !ok {
            fails.push(format!(
                "value={} opts={} expected={} got={:?}",
                c["value"], o, exp, got
            ));
        }
    }
    report("dumps", cases.len(), fails);
}

#[test]
fn golden_presets_match_fixture_opts() {
    // The named presets must equal the option sets the fixtures were recorded with.
    let pairs = [
        (
            DumpOpts::sorted_default_str(),
            (true, true, None, false, true, true),
        ),
        (DumpOpts::default(), (false, true, None, false, false, true)),
        (DumpOpts::sorted(), (true, true, None, false, false, true)),
        (
            DumpOpts::compact_sorted(),
            (true, true, Some((",", ":")), false, false, true),
        ),
        (
            DumpOpts::unicode(),
            (false, false, None, false, false, true),
        ),
        (
            DumpOpts::unicode_default_str(),
            (false, false, None, false, true, true),
        ),
        (
            DumpOpts::default_str(),
            (false, true, None, false, true, true),
        ),
        (
            DumpOpts::fastapi(),
            (false, false, Some((",", ":")), false, false, false),
        ),
    ];
    for (o, (sk, ea, sep, ind, ds, an)) in pairs {
        assert_eq!(o.sort_keys, sk);
        assert_eq!(o.ensure_ascii, ea);
        assert_eq!(
            o.separators.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
            sep
        );
        assert_eq!(o.indent.is_some(), ind);
        assert_eq!(o.default_str, ds);
        assert_eq!(o.allow_nan, an);
    }
}

#[test]
fn golden_str() {
    let cases = load("str.json");
    let mut fails = vec![];
    for c in &cases {
        let got = str_value(&decode(&c["value"]));
        if got != c["expected"].as_str().unwrap() {
            fails.push(format!(
                "value={} expected={} got={got:?}",
                c["value"], c["expected"]
            ));
        }
    }
    report("str", cases.len(), fails);
}

#[test]
fn golden_isoformat() {
    let cases = load("isoformat.json");
    let mut fails = vec![];
    for c in &cases {
        let got = match decode(&c["value"]) {
            PyValue::DateTime(d) => d.isoformat(),
            PyValue::Date(d) => date_isoformat(d),
            PyValue::Time(t) => t.isoformat(),
            other => panic!("unexpected {other:?}"),
        };
        if got != c["expected"].as_str().unwrap() {
            fails.push(format!(
                "value={} expected={} got={got:?}",
                c["value"], c["expected"]
            ));
        }
    }
    report("isoformat", cases.len(), fails);
}

/// DST gaps and folds: utcoffset/tzname/fold and the UTC instant (PEP 495 semantics of ZoneInfo).
#[test]
fn golden_dst() {
    let cases = load("dst.json");
    let mut fails = vec![];
    for c in &cases {
        let PyValue::DateTime(d) = decode(&c["value"]) else {
            panic!("not a datetime")
        };
        let got = (
            d.utcoffset().unwrap() as i64,
            d.tzname(),
            d.fold() as u64,
            d.to_utc_naive().format("%Y-%m-%dT%H:%M:%S%.6f").to_string(),
        );
        let exp = (
            c["utcoffset"].as_i64().unwrap(),
            c["tzname"].as_str().unwrap().to_string(),
            c["fold"].as_u64().unwrap(),
            c["utc"].as_str().unwrap().to_string(),
        );
        if got != exp {
            fails.push(format!("value={} expected={exp:?} got={got:?}", c["value"]));
        }
    }
    report("dst", cases.len(), fails);
}

#[test]
fn golden_float_repr() {
    let cases = load("float_repr.json");
    let mut fails = vec![];
    for c in &cases {
        let f = f64::from_bits(u64::from_str_radix(c["bits"].as_str().unwrap(), 16).unwrap());
        let got = py_float_repr(f);
        if got != c["expected"].as_str().unwrap() {
            fails.push(format!(
                "bits={} expected={} got={got:?}",
                c["bits"], c["expected"]
            ));
        }
    }
    report("float_repr", cases.len(), fails);
}

#[test]
fn golden_repr() {
    let cases = load("repr.json");
    let mut fails = vec![];
    for c in &cases {
        let v = decode(&c["value"]);
        let got = repr(&v);
        if got.as_deref() != Ok(c["expected"].as_str().unwrap()) {
            fails.push(format!(
                "value={} expected={} got={got:?}",
                c["value"], c["expected"]
            ));
        }
        if let PyValue::List(items) = &v {
            assert_eq!(repr_list(items), got);
        }
    }
    report("repr", cases.len(), fails);
}

#[test]
fn golden_decimal_encoder() {
    let cases = load("decimal_encoder.json");
    let mut fails = vec![];
    for c in &cases {
        let PyValue::Decimal(d) = decode(&c["value"]) else {
            panic!()
        };
        let got = decimal_encoder(&d);
        if got.as_deref() != Ok(c["expected"].as_str().unwrap()) {
            fails.push(format!(
                "value={} expected={} got={got:?}",
                c["value"], c["expected"]
            ));
        }
    }
    report("decimal_encoder", cases.len(), fails);
}

#[test]
fn rust_decimal_round_trip() {
    for s in [
        "0",
        "0.00",
        "1.10",
        "-0.50",
        "79228162514264337593543950335",
        "0.0000000000000000000000000001",
    ] {
        let d: rust_decimal::Decimal = s.parse().unwrap();
        let p = PyDecimal::from(d);
        assert_eq!(p.to_rust_decimal(), Some(d));
        assert_eq!(
            p.to_string(),
            if s.starts_with("0.000000000") {
                "1E-28"
            } else {
                s
            }
        );
    }
    assert_eq!(PyDecimal::parse("1E+2").unwrap().to_rust_decimal(), None);
}
