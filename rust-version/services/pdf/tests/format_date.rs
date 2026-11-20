//! The formatDate helper against recorded expectations (tests/fixtures/format_date_cases.json) in UTC and
//! in Asia/Tokyo. The cases were recorded once from V8's date parser (ISO and legacy forms) and
//! `toLocaleDateString("en-GB", { day: "2-digit", month: "long", year: "numeric" })`; nothing runs Node here.
//! The local time zone is read once per process, so each zone runs in a child process of this test.

use granary_pdf::dates::format_date;
use serde_json::Value;

fn check_zone(tz: &str) -> Vec<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/format_date_cases.json");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut failures = Vec::new();
    for (input, want) in cases["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .zip(cases[tz].as_array().unwrap())
    {
        let got = format_date(Some(input));
        if got != want.as_str().unwrap() {
            failures.push(format!("{tz} {input}: got {got:?}, expected {want}"));
        }
    }
    failures
}

#[test]
fn format_date_matches_recorded_cases_in_two_time_zones() {
    if let Ok(tz) = std::env::var("FORMAT_DATE_ZONE") {
        let failures = check_zone(&tz);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        return;
    }
    for tz in ["UTC", "Asia/Tokyo"] {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "format_date_matches_recorded_cases_in_two_time_zones",
                "--exact",
                "--nocapture",
            ])
            .env("TZ", tz)
            .env("FORMAT_DATE_ZONE", tz)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{tz}:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
