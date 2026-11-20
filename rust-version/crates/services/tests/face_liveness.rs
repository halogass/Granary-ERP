//! Port of backend/tests/test_face_liveness.py: pure tests for the active liveness check and the 1:N
//! search (no DB, no engine). The descriptors come from face_fixtures.py (numpy PCG64 draws), recorded in
//! compat-tests/golden/face/test_inputs.json by gen_face.py; `landmarks()` is ported as is.

use granary_services::face::{liveness, matching};
use serde_json::{json, Value};

fn inputs() -> Value {
    let p = format!(
        "{}/../../compat-tests/golden/face/test_inputs.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// face_fixtures.landmarks(eye_open=, yaw_shift=)
fn landmarks(eye_open: f64, yaw_shift: f64) -> Vec<Value> {
    let mut pts = vec![[0.0f64, 0.0]; 68];
    for (i, p) in pts.iter_mut().enumerate().take(17) {
        *p = [
            100.0 + i as f64 * 12.5,
            200.0 + (i as f64 - 8.0).abs() * -5.0,
        ];
    }
    for (i, p) in pts.iter_mut().enumerate().take(36).skip(27) {
        *p = [200.0 + yaw_shift, 170.0 + (i as f64 - 27.0) * 5.0];
    }
    pts[30] = [200.0 + yaw_shift, 205.0];
    for (base, x0) in [(36usize, 130.0), (42, 230.0)] {
        pts[base] = [x0, 150.0];
        pts[base + 1] = [x0 + 10.0, 150.0 - eye_open];
        pts[base + 2] = [x0 + 30.0, 150.0 - eye_open];
        pts[base + 3] = [x0 + 40.0, 150.0];
        pts[base + 4] = [x0 + 30.0, 150.0 + eye_open];
        pts[base + 5] = [x0 + 10.0, 150.0 + eye_open];
    }
    for (i, p) in pts.iter_mut().enumerate().skip(48) {
        *p = [170.0 + (i as f64 - 48.0) * 3.0, 240.0];
    }
    pts.iter().map(|p| json!(p)).collect()
}

fn frames(v: &Value) -> Vec<Value> {
    v.as_array().unwrap().clone()
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn fl(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

#[test]
fn test_eye_aspect_ratio_and_yaw_from_landmarks() {
    assert!(approx(
        liveness::eye_aspect_ratio(&landmarks(6.0, 0.0)).unwrap(),
        0.3,
        0.01
    ));
    assert!(approx(
        liveness::eye_aspect_ratio(&landmarks(2.0, 0.0)).unwrap(),
        0.1,
        0.01
    ));
    assert!(approx(
        liveness::yaw(&landmarks(6.0, 0.0)).unwrap(),
        0.0,
        0.01
    ));
    assert!(approx(
        liveness::yaw(&landmarks(6.0, 60.0)).unwrap(),
        0.3,
        0.01
    ));
}

#[test]
fn test_each_action_passes_when_performed() {
    let inp = inputs();
    for action in liveness::ACTIONS {
        let (passed, reason, metrics) =
            liveness::check(action, &frames(&inp["each_action"][action])).unwrap();
        assert!(passed, "{reason:?}");
        let m = liveness::metrics_json(&metrics);
        let mut keys: Vec<&String> = m.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["ear", "same_face", "yaw"]);
    }
}

#[test]
fn test_a_still_photo_fails_every_action() {
    let still = frames(&inputs()["still"]);
    for action in liveness::ACTIONS {
        assert!(!liveness::check(action, &still).unwrap().0);
    }
}

#[test]
fn test_turning_the_wrong_way_fails() {
    let inp = inputs();
    let left = frames(&inp["wrong_way"]["turn_left"]);
    let right = frames(&inp["wrong_way"]["turn_right"]);
    assert!(!liveness::check("turn_right", &left).unwrap().0);
    assert!(!liveness::check("turn_left", &right).unwrap().0);
    assert!(!liveness::check("blink", &left).unwrap().0);
}

#[test]
fn test_two_faces_or_a_small_face_are_refused() {
    let inp = inputs();
    let (passed, reason, _) = liveness::check("blink", &frames(&inp["two_faces"])).unwrap();
    assert!(!passed && reason.unwrap().contains("More than one face"));
    assert!(liveness::check("blink", &frames(&inp["small"]))
        .unwrap()
        .1
        .unwrap()
        .contains("closer"));
}

#[test]
fn test_a_different_person_mid_check_fails() {
    let (passed, reason, _) = liveness::check("blink", &frames(&inputs()["changed"])).unwrap();
    assert!(!passed && reason.unwrap().contains("face changed"));
}

#[test]
fn test_search_picks_the_closest_person_and_flags_ambiguity() {
    let inp = inputs();
    let s = &inp["search"];
    let ids: Vec<String> = ["a", "a", "b"].iter().map(|x| x.to_string()).collect();
    let matrix: Vec<Vec<f64>> = s["matrix"].as_array().unwrap().iter().map(fl).collect();
    let q = fl(&s["query"]);
    let found = matching::search_in(&ids, &matrix, &[&q]);
    assert!(
        found.employee_id.as_deref() == Some("a") && found.runner_up_id.as_deref() == Some("b")
    );
    assert!(found.accepted(0.5) && !found.ambiguous(0.5));
    let stranger = fl(&s["stranger"]);
    assert!(!matching::search_in(&ids, &matrix, &[&stranger]).accepted(0.5));
    // two enrolled people who look nearly identical -> never auto-accepted
    let twin_ids: Vec<String> = vec!["a".into(), "twin".into()];
    let twin_matrix: Vec<Vec<f64>> = s["twin_matrix"]
        .as_array()
        .unwrap()
        .iter()
        .map(fl)
        .collect();
    let twin = matching::search_in(&twin_ids, &twin_matrix, &[&q]);
    assert!(twin.ambiguous(0.5) && !twin.accepted(0.5));
}

#[test]
fn test_descriptor_validation() {
    assert!(matching::valid_descriptor(&fl(&inputs()["valid"])));
    assert!(!matching::valid_descriptor(&[0.1; 127]));
    assert!(!matching::valid_descriptor(&[f64::NAN; 128]));
    assert!(!matching::valid_descriptor_value(&json!("nope")));
}

#[test]
fn only_head_turns_are_issued_but_a_blink_is_still_judged() {
    assert_eq!(liveness::ISSUED_ACTIONS, ["turn_left", "turn_right"]);
    assert!(liveness::ISSUED_ACTIONS
        .iter()
        .all(|a| liveness::ACTIONS.contains(a)));
    assert!(liveness::ACTIONS.contains(&"blink"));
}
