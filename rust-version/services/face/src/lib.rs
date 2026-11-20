//! The Granary face engine sidecar in Rust: the HTTP contract of services/face (face-api.js 1.7.15 on
//! tfjs-node), the same networks and weights on ONNX Runtime (models/convert.py).

pub mod antispoof;
pub mod decode;
pub mod http;
pub mod jsnum;
pub mod log;
pub mod pipeline;

use std::path::PathBuf;

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The face-api npm package folder (dist/face-api.esm.js, model/*): FACE_API_DIR, else ./face-api when
/// present, else the crate's models/face-api (development; see models/README.md).
pub fn package_dir() -> PathBuf {
    if let Some(d) = env_dir("FACE_API_DIR") {
        return d;
    }
    let local = PathBuf::from("face-api");
    if local.join("model").is_dir() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join("face-api")
}

/// The converted ONNX models: FACE_ONNX_DIR, else ./onnx when present, else the crate's models/onnx.
pub fn onnx_dir() -> PathBuf {
    if let Some(d) = env_dir("FACE_ONNX_DIR") {
        return d;
    }
    let local = PathBuf::from("onnx");
    if local.join(pipeline::SSD_MODEL).is_file() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join("onnx")
}

/// The committed anti-spoof models (models/antispoof): FACE_ANTISPOOF_DIR, else ./antispoof when present,
/// else the crate's models/antispoof.
pub fn antispoof_dir() -> PathBuf {
    if let Some(d) = env_dir("FACE_ANTISPOOF_DIR") {
        return d;
    }
    let local = PathBuf::from("antispoof");
    if local.join(antispoof::MODEL_V2).is_file() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join("antispoof")
}

/// `Number(process.env.FACE_MAX_CONCURRENT || 2)` as withSlot uses it: Some(slots), None for no limit
/// (NaN never compares true). Zero or less would block every request in Node; here it means one slot.
pub fn max_concurrent(v: Option<&str>) -> Option<usize> {
    let v = v.filter(|s| !s.is_empty()).unwrap_or("2").trim();
    let n: f64 = if v.is_empty() {
        0.0
    } else if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        i64::from_str_radix(h, 16)
            .map(|x| x as f64)
            .unwrap_or(f64::NAN)
    } else {
        v.parse().unwrap_or(f64::NAN)
    };
    if n.is_nan() || n == f64::INFINITY {
        None
    } else {
        Some((n.ceil().max(1.0)) as usize)
    }
}
