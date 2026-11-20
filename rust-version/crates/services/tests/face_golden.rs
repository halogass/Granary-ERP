//! Golden fixtures recorded from the real Python face functions (compat-tests/golden/gen_face.py):
//! bit-for-bit floats (math.hypot, builtin sum, round, numpy norm) and identical decisions.

use granary_services::face::{liveness, matching};
use serde_json::Value;

fn load(name: &str) -> Value {
    let p = format!(
        "{}/../../compat-tests/golden/face/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}")))
        .unwrap()
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a float: {v}"))
}

fn fl(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(f).collect()
}

fn same_bits(a: f64, b: f64, ctx: &str) {
    assert!(
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()),
        "{ctx}: rust {a:?} python {b:?}"
    );
}

#[test]
fn primitives_hypot_round_sum() {
    let p = load("primitives");
    let hyp = p["hypot"].as_array().unwrap();
    for c in hyp {
        let (a, b) = (f(&c[0]), f(&c[1]));
        same_bits(
            liveness::py_vector_norm(&[a, b]),
            f(&c[2]),
            &format!("hypot({a:?}, {b:?})"),
        );
    }
    for c in p["round"].as_array().unwrap() {
        let x = f(&c[0]);
        for (i, n) in [2usize, 3, 4].iter().enumerate() {
            same_bits(
                liveness::py_round(x, *n),
                f(&c[i + 1]),
                &format!("round({x:?}, {n})"),
            );
        }
    }
    for c in p["descriptor_distance"].as_array().unwrap() {
        same_bits(
            liveness::descriptor_distance(&fl(&c[0]), &fl(&c[1])),
            f(&c[2]),
            "descriptor_distance",
        );
    }
    assert!(hyp.len() > 4000);
}

#[test]
fn landmarks_ear_and_yaw() {
    for (i, c) in load("landmarks").as_array().unwrap().iter().enumerate() {
        let lms = c[0].as_array().unwrap();
        same_bits(
            liveness::eye_aspect_ratio(lms).unwrap(),
            f(&c[1]),
            &format!("ear #{i}"),
        );
        same_bits(liveness::yaw(lms).unwrap(), f(&c[2]), &format!("yaw #{i}"));
    }
}

#[test]
fn liveness_check_decisions_and_metrics() {
    let cases = load("check");
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        let images = c["images"].as_array().unwrap();
        let (passed, reason, metrics) =
            liveness::check(c["action"].as_str().unwrap(), images).unwrap();
        assert_eq!(passed, c["passed"].as_bool().unwrap(), "case {i}");
        assert_eq!(reason, c["reason"].as_str(), "case {i}");
        let got = liveness::metrics_json(&metrics);
        let want = &c["metrics"];
        assert_eq!(
            got.as_object().unwrap().keys().collect::<Vec<_>>(),
            want.as_object().unwrap().keys().collect::<Vec<_>>(),
            "case {i}"
        );
        for (k, v) in want.as_object().unwrap() {
            for (a, b) in fl(&got[k]).iter().zip(fl(v)) {
                same_bits(*a, b, &format!("case {i} metric {k}"));
            }
        }
    }
}

#[test]
fn search_norms_matches_and_decisions() {
    for (i, c) in load("search").as_array().unwrap().iter().enumerate() {
        let ids: Vec<String> = c["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let matrix: Vec<Vec<f64>> = c["matrix"].as_array().unwrap().iter().map(fl).collect();
        let queries: Vec<Vec<f64>> = c["queries"].as_array().unwrap().iter().map(fl).collect();
        for (q, norms) in queries.iter().zip(c["norms"].as_array().unwrap()) {
            for (row, n) in matrix.iter().zip(fl(norms)) {
                same_bits(
                    matching::np_row_distance(row, q),
                    n,
                    &format!("case {i} norm"),
                );
            }
        }
        let qs: Vec<&[f64]> = queries.iter().map(|q| q.as_slice()).collect();
        let m = matching::search_in(&ids, &matrix, &qs);
        let want = c["match"].as_array().unwrap();
        assert_eq!(m.employee_id.as_deref(), want[0].as_str(), "case {i}");
        assert_eq!(m.runner_up_id.as_deref(), want[2].as_str(), "case {i}");
        for (got, w) in [(m.distance, &want[1]), (m.runner_up_distance, &want[3])] {
            match got {
                None => assert!(w.is_null(), "case {i}"),
                Some(g) => same_bits(g, f(w), &format!("case {i} distance")),
            }
        }
        for (t, d) in c["decisions"].as_object().unwrap() {
            let t: f64 = t.parse().unwrap();
            assert_eq!(
                m.accepted(t),
                d[0].as_bool().unwrap(),
                "case {i} accepted({t})"
            );
            assert_eq!(
                m.ambiguous(t),
                d[1].as_bool().unwrap(),
                "case {i} ambiguous({t})"
            );
        }
    }
}

#[test]
fn valid_descriptor_cases() {
    for c in load("valid").as_array().unwrap() {
        assert_eq!(
            matching::valid_descriptor_value(&c[0]),
            c[1].as_bool().unwrap(),
            "{}",
            c[0]
        );
    }
    // non-JSON floats, inline
    assert!(!matching::valid_descriptor(&[f64::NAN; 128]));
    let mut inf = vec![0.0; 128];
    inf[0] = f64::INFINITY;
    assert!(!matching::valid_descriptor(&inf));
    assert!(matching::valid_descriptor(&[0.1; 128]));
    assert!(!matching::valid_descriptor(&[0.1; 127]));
}
