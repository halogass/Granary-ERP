//! The Owner's face data set through the Rust engine, against the aggregate numbers face-api.js (Node) gave
//! on the same photos (compat-tests/services/face/summary.json, `originals_node`, written by run_both.sh).
//!
//! PRIVACY: the photos are read in place (FACE_DATASET_DIR; the test skips when it is unset) and nothing is written;
//! only counts are compared. The test skips when the folder or the converted models are absent.

use std::path::{Path, PathBuf};

use granary_face::decode;
use granary_face::http::MAX_FILE_SIZE;
use granary_face::jsnum::round;
use granary_face::pipeline::{Engine, SSD_MODEL};

const NO_PERSON: &str = "Tower";

fn dist(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt()
}

/// The photos the aggregates were recorded on (compat-tests/services/face/dataset_files.txt): photos added to
/// the folder later must not change the numbers (10 Nov 2026: 4 photos added on 9 Nov failed the test).
fn recorded() -> std::collections::HashSet<String> {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../compat-tests/services/face/dataset_files.txt"),
    )
    .unwrap()
    .lines()
    .map(str::trim)
    .filter(|l| !l.is_empty() && !l.starts_with('#'))
    .map(str::to_string)
    .collect()
}

fn files(root: &Path) -> Vec<(String, PathBuf)> {
    let keep = recorded();
    let mut dirs: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    dirs.sort_by_key(|e| e.file_name());
    let mut out = Vec::new();
    for d in dirs {
        let mut fs: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .collect();
        fs.sort_by_key(|e| e.file_name());
        for f in fs {
            let rel = format!(
                "{}/{}",
                d.file_name().to_string_lossy(),
                f.file_name().to_string_lossy()
            );
            if keep.contains(&rel) {
                out.push((d.file_name().to_string_lossy().into_owned(), f.path()));
            }
        }
    }
    out
}

#[test]
fn dataset_matches_face_api_aggregates() {
    let root = PathBuf::from(std::env::var("FACE_DATASET_DIR").unwrap_or_default());
    let models = granary_face::onnx_dir();
    if !root.is_dir() || !models.join(SSD_MODEL).is_file() {
        eprintln!(
            "SKIP: data set ({}) or models ({}) absent",
            root.display(),
            models.display()
        );
        return;
    }
    let summary: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compat-tests/services/face/summary.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected = &summary["originals_node"];
    let engine = Engine::load(&models).unwrap();

    // the photos the service analyses as sent: within multer's limit and decodable
    let (mut photos, mut faces, mut no_person) = (0u64, 0u64, 0u64);
    let mut main: Vec<(String, Vec<f64>)> = Vec::new();
    for (person, path) in files(&root) {
        let bytes = std::fs::read(&path).unwrap();
        if bytes.len() >= MAX_FILE_SIZE {
            continue;
        }
        let Ok(d) = decode::decode(&bytes) else {
            continue;
        };
        let mut fs = engine.analyze(&d.image, d.batch).unwrap();
        photos += 1;
        faces += fs.len() as u64;
        if person == NO_PERSON {
            no_person += fs.len() as u64;
            continue;
        }
        // the service's order: largest (rounded) box first
        fs.sort_by(|a, b| {
            let area = |f: &granary_face::pipeline::Face| {
                round(f.detection.width, 2) * round(f.detection.height, 2)
            };
            area(b).partial_cmp(&area(a)).unwrap()
        });
        if let Some(f) = fs.first() {
            let desc = f.descriptor.iter().map(|&v| round(v as f64, 6)).collect();
            main.push((person, desc));
        }
    }
    let mut got = serde_json::Map::new();
    got.insert("photos".into(), photos.into());
    got.insert("faces".into(), faces.into());
    got.insert("no_person_faces".into(), no_person.into());
    let mut counts = std::collections::BTreeMap::<String, u64>::new();
    for i in 0..main.len() {
        for j in i + 1..main.len() {
            let kind = if main[i].0 == main[j].0 {
                "same"
            } else {
                "different"
            };
            *counts.entry(format!("{kind}_pairs")).or_default() += 1;
            let d = dist(&main[i].1, &main[j].1);
            for t in [0.50, 0.60] {
                *counts.entry(format!("{kind}_match_{t:.2}")).or_default() += u64::from(d <= t);
            }
        }
    }
    for (k, v) in counts {
        got.insert(k, v.into());
    }
    assert_eq!(&serde_json::Value::Object(got), expected);
}
