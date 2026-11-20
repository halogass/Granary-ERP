//! Every /render fixture in tests/fixtures/ renders to the HTML recorded in
//! tests/fixtures/golden_html/<stem>.html (TZ=UTC). The goldens are recorded from this renderer and catch
//! unintended template or helper changes. After a change you meant, run with `GOLDEN_BLESS=1` to write the
//! goldens again, then review their diff.

use std::path::PathBuf;

use granary_pdf::hbs::Templates;
use serde_json::Value;

/// JSON files in tests/fixtures/ that are not /render fixtures.
const NOT_RENDER_FIXTURES: &[&str] = &["format_date_cases.json"];

fn blessing() -> bool {
    std::env::var("GOLDEN_BLESS").is_ok_and(|v| v == "1")
}

#[test]
fn templates_render_byte_identical_html() {
    // formatDate formats in the process time zone; the golden files are rendered with TZ=UTC.
    std::env::set_var("TZ", "UTC");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let suite = root.join("tests/fixtures");
    let golden = suite.join("golden_html");
    let templates = Templates::new(root.join("assets/templates"));
    let mut names: Vec<PathBuf> = std::fs::read_dir(&suite)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            !NOT_RENDER_FIXTURES.contains(&name.as_ref())
        })
        .collect();
    names.sort();
    assert!(names.len() >= 15, "fixtures missing");
    if blessing() {
        std::fs::create_dir_all(&golden).unwrap();
    }
    let mut failures = Vec::new();
    for path in &names {
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let got = templates.render(&body["template"], &body["data"]).unwrap();
        let golden_path = golden.join(format!("{stem}.html"));
        if blessing() {
            std::fs::write(&golden_path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!(
                "{}: {e} (run with GOLDEN_BLESS=1 to record it)",
                golden_path.display()
            )
        });
        if got != want {
            let at = got
                .bytes()
                .zip(want.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(got.len().min(want.len()));
            let lo = at.saturating_sub(80);
            failures.push(format!(
                "{stem}: first difference at byte {at}\n  got:    {:?}\n  golden: {:?}",
                &got[lo..(at + 80).min(got.len())],
                &want[lo..(at + 80).min(want.len())]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
