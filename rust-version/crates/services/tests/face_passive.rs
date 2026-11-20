//! The passive liveness flash maths against compat-tests/golden/face/passive_flash.json (written by
//! an independent plain-Python reference (not shipped); the kiosk's TypeScript port reads the same
//! file in its vitest).

use granary_services::face::flash::{chroma, judge, verdict, Outcome, ZERO_VARIANCE};
use serde_json::Value;

fn golden() -> Value {
    let p = format!(
        "{}/../../compat-tests/golden/face/passive_flash.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}")))
        .unwrap()
}

fn rgb(v: &Value) -> [f64; 3] {
    let a = v.as_array().unwrap();
    [
        a[0].as_f64().unwrap(),
        a[1].as_f64().unwrap(),
        a[2].as_f64().unwrap(),
    ]
}

fn rgbs(v: &Value) -> Vec<[f64; 3]> {
    v.as_array().unwrap().iter().map(rgb).collect()
}

fn outcome(s: &str) -> Outcome {
    match s {
        "pass" => Outcome::Pass,
        "fail" => Outcome::Fail,
        _ => Outcome::Inconclusive,
    }
}

#[test]
fn flash_judgement_matches_the_golden_vectors() {
    let g = golden();
    assert_eq!(g["zero_variance_eps"].as_f64(), Some(ZERO_VARIANCE));
    let cases = g["cases"].as_array().unwrap();
    assert!(cases.len() >= 5);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let j = judge(
            rgb(&c["neutral"]),
            &rgbs(&c["flashes"]),
            &rgbs(&c["colors"]),
            c["flash_min_delta"].as_f64().unwrap(),
            c["flash_min_corr"].as_f64().unwrap(),
        );
        let e = &c["expected"];
        assert_eq!(j.outcome.as_str(), e["outcome"].as_str().unwrap(), "{name}");
        assert!(
            (j.corr - e["corr"].as_f64().unwrap()).abs() < 1e-9,
            "{name}: corr {} vs {}",
            j.corr,
            e["corr"]
        );
        assert!(
            (j.delta - e["delta"].as_f64().unwrap()).abs() < 1e-9,
            "{name}: delta {} vs {}",
            j.delta,
            e["delta"]
        );
        assert_eq!(
            j.lag as u64,
            e["lag"].as_u64().unwrap(),
            "{name}: lag {}",
            j.lag
        );
    }
    // the intent of the named cases
    let by = |n: &str| {
        cases
            .iter()
            .find(|c| c["name"] == n)
            .unwrap_or_else(|| panic!("{n}"))["expected"]["outcome"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(by("clear_pass"), "pass");
    assert_ne!(by("printed_uniform"), "pass");
    assert_eq!(by("low_delta"), "inconclusive");
    assert_eq!(by("wrong_order"), "fail");
    assert_eq!(by("zero_variance"), "inconclusive");
    assert_eq!(by("camera_late"), "pass");
    assert_eq!(by("exposure_change"), "pass");
    let lag = |n: &str| {
        cases.iter().find(|c| c["name"] == n).unwrap()["expected"]["lag"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(lag("camera_late"), 1);
    assert_eq!(lag("clear_pass"), 0);
    assert_eq!(lag("two_flashes"), 0);
}

#[test]
fn chromaticity_ignores_brightness() {
    let a = chroma([150.0, 100.0, 90.0]);
    let b = chroma([75.0, 50.0, 45.0]);
    for c in 0..3 {
        assert!((a[c] - b[c]).abs() < 1e-12);
    }
    assert!((a.iter().sum::<f64>() - 255.0).abs() < 1e-9);
    assert_eq!(chroma([0.0; 3]), [0.0; 3]);
    // every flash frame only darker or brighter than the neutral one: no colour response at all
    let n = [150.0, 100.0, 90.0];
    let flashes: Vec<[f64; 3]> = [0.7, 1.1, 0.8, 1.2, 0.9]
        .iter()
        .map(|g| [n[0] * g, n[1] * g, n[2] * g])
        .collect();
    let colors = [
        [255.0, 120.0, 120.0],
        [120.0, 255.0, 140.0],
        [120.0, 150.0, 255.0],
        [255.0, 230.0, 120.0],
        [220.0, 130.0, 255.0],
    ];
    let j = judge(n, &flashes, &colors, 1.5, 0.6);
    assert_eq!(j.outcome, Outcome::Inconclusive);
    assert!(j.delta < 1e-9);
}

#[test]
fn verdicts_match_the_golden_vectors() {
    let g = golden();
    for v in g["verdicts"].as_array().unwrap() {
        let r = verdict(
            outcome(v["flash"].as_str().unwrap()),
            v["spoof_real"].as_f64().unwrap(),
            v["spoof_min_real"].as_f64().unwrap(),
        );
        let e = &v["expected"];
        assert_eq!(r.passed(), e["passed"].as_bool().unwrap(), "{v}");
        assert_eq!(r.reason(), e["reason"].as_str(), "{v}");
        assert_eq!(r.attempt_outcome(), e["attempt_outcome"].as_str(), "{v}");
    }
}

#[test]
fn mismatched_lengths_are_inconclusive() {
    let j = judge([1.0; 3], &[[2.0; 3]], &[], 4.0, 0.6);
    assert_eq!(j.outcome, Outcome::Inconclusive);
}
