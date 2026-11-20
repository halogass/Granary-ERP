//! Port of backend/app/services/face/liveness.py.
//!
//! Active liveness from 68-point landmarks (iBUG layout, as face-api.js returns them).
//!
//! The user performs one random action between two neutral frames: frame 0 neutral, frame 1 the action,
//! frame 2 neutral again. A photo held to the camera cannot blink or turn, so it fails.
//!
//! Coordinates are those of the raw (un-mirrored) camera frame. When a person turns their head to THEIR
//! left, the nose moves toward the image's right, so yaw increases. The same formulas and thresholds are
//! mirrored in frontend/src/lib/faceLiveness.ts for the kiosk and for on-screen guidance.
//!
//! Float maths is bit-identical to CPython 3.12: `math.hypot` is CPython's `vector_norm` (scaled,
//! double-length accumulation, one correction step), `sum()` of floats is Neumaier's compensated sum, and
//! `round(x, n)` is the correctly rounded (half-even on the exact binary value) decimal. Golden-tested in
//! tests/face_golden.rs against fixtures recorded from the real Python functions.
//!
//! Frames are the sidecar's JSON (`{width, height, faces: [{score, box, landmarks, descriptor}]}`). Where
//! Python would raise KeyError/TypeError on a malformed frame (a 500), these return an `anyhow` error.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// Every action `check` can judge (golden-frozen: old nonces and fixtures still carry "blink").
pub const ACTIONS: [&str; 3] = ["blink", "turn_left", "turn_right"];
/// The actions new challenges are issued with (6 Nov 2026, Owner: blink was never reliably detected on phones,
/// so only head turns are asked for; `check` still judges a blink).
pub const ISSUED_ACTIONS: [&str; 2] = ["turn_left", "turn_right"];

pub const MIN_FACE_SCORE: f64 = 0.8;
pub const MIN_FACE_PX: f64 = 120.0;
/// Eye aspect ratio of an open eye.
pub const EYE_OPEN_MIN: f64 = 0.20;
/// A closed eye's EAR is at most this fraction of the open EAR.
pub const BLINK_RATIO: f64 = 0.70;
/// |yaw| of a neutral, facing-the-camera frame.
pub const FRONTAL_MAX_YAW: f64 = 0.12;
/// Yaw change for a head turn.
pub const TURN_MIN_DELTA: f64 = 0.20;
/// Descriptor distance between the two neutral frames.
pub const SAME_FACE_NEUTRAL: f64 = 0.45;
/// ...and between a neutral and the action frame (eyes shut / head turned).
pub const SAME_FACE_ACTION: f64 = 0.55;

// ─── CPython float primitives ────────────────────────────────────────────────

/// `round(x, ndigits)` for a float (ndigits >= 0): Rust's exact fixed formatting rounds the exact binary
/// value half-to-even, as CPython's `_Py_dg_dtoa(mode 3)` does; parsing back is the same strtod.
pub fn py_round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}")
        .parse()
        .expect("formatted float parses")
}

/// CPython `vector_norm` (Modules/mathmodule.c, 3.12): what `math.hypot(*coords)` returns.
pub fn py_vector_norm(coords: &[f64]) -> f64 {
    let mut max = 0.0f64;
    let mut found_nan = false;
    let mut vec: Vec<f64> = Vec::with_capacity(coords.len());
    for &c in coords {
        let x = c.abs();
        found_nan |= x.is_nan();
        if x > max {
            max = x;
        }
        vec.push(x);
    }
    vector_norm(&mut vec, max, found_nan)
}

fn frexp_exp(x: f64) -> i32 {
    // exponent e with x = m * 2^e, 0.5 <= m < 1 (x finite, > 0)
    if x == 0.0 {
        return 0;
    }
    let bits = x.to_bits();
    let raw = ((bits >> 52) & 0x7ff) as i32;
    if raw == 0 {
        // subnormal: normalise
        let (m, e) = (x * 2f64.powi(54), -54);
        return frexp_exp(m) + e;
    }
    raw - 1022
}

fn ldexp(x: f64, e: i32) -> f64 {
    // exact for the powers of two used here (|e| <= 1074)
    if e > 1023 {
        x * 2f64.powi(1023) * 2f64.powi(e - 1023)
    } else if e < -1022 {
        x * 2f64.powi(-1022) * 2f64.powi(e + 1022)
    } else {
        x * 2f64.powi(e)
    }
}

fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let z = x - a;
    (x, b - z)
}

fn vector_norm(vec: &mut [f64], max: f64, found_nan: bool) -> f64 {
    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || vec.len() <= 1 {
        return max;
    }
    let max_e = frexp_exp(max);
    if max_e < -1023 {
        for v in vec.iter_mut() {
            *v /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vec, max / f64::MIN_POSITIVE, found_nan);
    }
    let scale = ldexp(1.0, -max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for &v in vec.iter() {
        let x = v * scale;
        let pr = dl_mul(x, x);
        let sm = dl_fast_sum(csum, pr.0);
        csum = sm.0;
        frac1 += pr.1;
        frac2 += sm.1;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.0);
    csum = sm.0;
    frac1 += pr.1;
    frac2 += sm.1;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

/// CPython 3.12 builtin `sum()` over floats (start 0): Neumaier compensated summation.
pub fn py_fsum_builtin(items: impl IntoIterator<Item = f64>) -> f64 {
    let mut f_result = 0.0f64;
    let mut c = 0.0f64;
    for x in items {
        let t = f_result + x;
        if f_result.abs() >= x.abs() {
            c += (f_result - t) + x;
        } else {
            c += (x - t) + f_result;
        }
        f_result = t;
    }
    if c != 0.0 && c.is_finite() {
        f_result += c;
    }
    f_result
}

// ─── Geometry ────────────────────────────────────────────────────────────────

fn num(v: &Value) -> Result<f64> {
    match v {
        Value::Number(n) => n.as_f64().ok_or_else(|| anyhow!("not a number: {n}")),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        other => Err(anyhow!("not a number: {other}")),
    }
}

fn point(lms: &[Value], i: usize) -> Result<(f64, f64)> {
    let p = lms
        .get(i)
        .ok_or_else(|| anyhow!("IndexError: landmark {i}"))?;
    let a = p
        .as_array()
        .ok_or_else(|| anyhow!("landmark {i} is not a list"))?;
    Ok((
        num(a.first().ok_or_else(|| anyhow!("IndexError"))?)?,
        num(a.get(1).ok_or_else(|| anyhow!("IndexError"))?)?,
    ))
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    py_vector_norm(&[a.0 - b.0, a.1 - b.1])
}

/// `descriptor_distance(a, b)`: `math.sqrt(sum((x - y) ** 2 for x, y in zip(a, b)))` (zip stops at the
/// shorter one).
pub fn descriptor_distance(a: &[f64], b: &[f64]) -> f64 {
    py_fsum_builtin(a.iter().zip(b).map(|(x, y)| {
        let d = x - y;
        d * d
    }))
    .sqrt()
}

fn descriptor_of(face: &Value) -> Result<Vec<f64>> {
    face.get("descriptor")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("KeyError: 'descriptor'"))?
        .iter()
        .map(num)
        .collect()
}

/// `eye_aspect_ratio(landmarks)`: mean EAR of both eyes, (|p2-p6| + |p3-p5|) / (2 |p1-p4|), points 36-41
/// and 42-47.
pub fn eye_aspect_ratio(landmarks: &[Value]) -> Result<f64> {
    let ear = |i: usize| -> Result<f64> {
        let p: Vec<(f64, f64)> = (i..i + 6)
            .map(|k| point(landmarks, k))
            .collect::<Result<_>>()?;
        let den = 2.0 * dist(p[0], p[3]);
        // Python `2 * d or 1e-6`: a zero width falls back to 1e-6
        let den = if den == 0.0 { 1e-6 } else { den };
        Ok((dist(p[1], p[5]) + dist(p[2], p[4])) / den)
    };
    Ok((ear(36)? + ear(42)?) / 2.0)
}

/// `yaw(landmarks)`: where the nose tip (30) sits between the jaw edges (0, 16); 0 = facing the camera.
pub fn yaw(landmarks: &[Value]) -> Result<f64> {
    let (left, right, nose) = (
        point(landmarks, 0)?,
        point(landmarks, 16)?,
        point(landmarks, 30)?,
    );
    let width = right.0 - left.0;
    Ok(if width.abs() > 1e-6 {
        (nose.0 - left.0) / width - 0.5
    } else {
        0.0
    })
}

/// `main_face(image)`: the single clear face in a frame, or why there isn't one.
pub fn main_face(image: &Value) -> Result<(Option<&Value>, Option<&'static str>)> {
    let mut faces = Vec::new();
    if let Some(list) = image.get("faces") {
        let list = list
            .as_array()
            .ok_or_else(|| anyhow!("faces is not a list"))?;
        for f in list {
            let score = num(f.get("score").ok_or_else(|| anyhow!("KeyError: 'score'"))?)?;
            if score >= MIN_FACE_SCORE {
                faces.push(f);
            }
        }
    }
    if faces.is_empty() {
        return Ok((None, Some("No face detected: look at the camera")));
    }
    if faces.len() > 1 {
        return Ok((
            None,
            Some("More than one face in view: only the person clocking in"),
        ));
    }
    let face = faces[0];
    let bx = face.get("box").ok_or_else(|| anyhow!("KeyError: 'box'"))?;
    let w = num(bx
        .get("width")
        .ok_or_else(|| anyhow!("KeyError: 'width'"))?)?;
    let h = num(bx
        .get("height")
        .ok_or_else(|| anyhow!("KeyError: 'height'"))?)?;
    if w.min(h) < MIN_FACE_PX {
        return Ok((None, Some("Move closer to the camera")));
    }
    Ok((Some(face), None))
}

/// The `metrics` dict of `check` (logged on the clock event). `None` is Python's `{}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub ear: [f64; 3],
    pub yaw: [f64; 3],
    pub same_face: [f64; 2],
}

impl Metrics {
    /// `{"ear": [...], "yaw": [...], "same_face": [...]}` in Python's key order.
    pub fn to_json(&self) -> Value {
        json!({"ear": self.ear, "yaw": self.yaw, "same_face": self.same_face})
    }
}

/// `metrics` as the dict Python returns (`{}` when the check stopped early).
pub fn metrics_json(m: &Option<Metrics>) -> Value {
    m.as_ref()
        .map(Metrics::to_json)
        .unwrap_or_else(|| json!({}))
}

/// `(passed, reason, metrics)`.
pub type CheckResult = (bool, Option<&'static str>, Option<Metrics>);

/// `check(action, images)`: did the person in these 3 frames perform `action`, and is it the same person
/// throughout?
pub fn check(action: &str, images: &[Value]) -> Result<CheckResult> {
    if !ACTIONS.contains(&action) {
        return Ok((false, Some("Unknown liveness action"), None));
    }
    if images.len() != 3 {
        return Ok((false, Some("Three frames are required"), None));
    }
    let mut faces = Vec::with_capacity(3);
    for image in images {
        let (face, reason) = main_face(image)?;
        match face {
            Some(f) => faces.push(f),
            None => return Ok((false, reason, None)),
        }
    }
    let mut ears = [0.0; 3];
    let mut yaws = [0.0; 3];
    for (i, f) in faces.iter().enumerate() {
        let lms = f
            .get("landmarks")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("KeyError: 'landmarks'"))?;
        ears[i] = eye_aspect_ratio(lms)?;
    }
    for (i, f) in faces.iter().enumerate() {
        let lms = f
            .get("landmarks")
            .and_then(Value::as_array)
            .expect("checked above");
        yaws[i] = yaw(lms)?;
    }
    let d: Vec<Vec<f64>> = faces
        .iter()
        .map(|f| descriptor_of(f))
        .collect::<Result<_>>()?;
    let d_neutral = descriptor_distance(&d[0], &d[2]);
    let d01 = descriptor_distance(&d[0], &d[1]);
    let d21 = descriptor_distance(&d[2], &d[1]);
    // Python max(a, b): the first unless the second is greater
    let d_action = if d21 > d01 { d21 } else { d01 };
    let metrics = Metrics {
        ear: ears.map(|e| py_round(e, 3)),
        yaw: yaws.map(|y| py_round(y, 3)),
        same_face: [py_round(d_neutral, 3), py_round(d_action, 3)],
    };

    if d_neutral > SAME_FACE_NEUTRAL || d_action > SAME_FACE_ACTION {
        return Ok((
            false,
            Some("The face changed during the check: try again"),
            Some(metrics),
        ));
    }
    if yaws[0].abs() > FRONTAL_MAX_YAW || yaws[2].abs() > FRONTAL_MAX_YAW {
        return Ok((
            false,
            Some("Face the camera before and after the action"),
            Some(metrics),
        ));
    }

    if action == "blink" {
        // Python min(a, b): the first unless the second is smaller
        let open_ear = if ears[2] < ears[0] { ears[2] } else { ears[0] };
        let passed = open_ear >= EYE_OPEN_MIN && ears[1] <= BLINK_RATIO * open_ear;
        return Ok((
            passed,
            if passed {
                None
            } else {
                Some("Blink not detected: try again")
            },
            Some(metrics),
        ));
    }
    let delta = yaws[1] - yaws[0];
    let passed = if action == "turn_left" {
        delta >= TURN_MIN_DELTA
    } else {
        delta <= -TURN_MIN_DELTA
    };
    Ok((
        passed,
        if passed {
            None
        } else {
            Some("Head turn not detected: try again")
        },
        Some(metrics),
    ))
}
