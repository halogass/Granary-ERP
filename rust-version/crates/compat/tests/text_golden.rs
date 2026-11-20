//! Golden tests for granary_compat::text against fixtures recorded from the real Python backend
//! (compat-tests/golden/gen_text.py). Every comparison is exact.

use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use granary_compat::text::{
    difflib, matching, patterns, phonelib, phones, pystr, relativedelta, utils,
};
use serde_json::Value;

fn load(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../compat-tests/golden/text/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap()
}

fn s(v: &Value) -> Option<&str> {
    v.as_str()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

fn f(v: &Value) -> f64 {
    v.as_str().unwrap().parse::<f64>().unwrap()
}

fn opt_string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// Collects mismatches so one run shows all of them.
#[derive(Default)]
struct Check {
    fails: Vec<String>,
    n: usize,
}

impl Check {
    fn eq<T: PartialEq + std::fmt::Debug>(
        &mut self,
        what: &str,
        input: impl std::fmt::Debug,
        got: T,
        want: T,
    ) {
        self.n += 1;
        if got != want {
            self.fails
                .push(format!("{what} {input:?}: got {got:?}, want {want:?}"));
        }
    }
    fn done(self, name: &str) {
        eprintln!("{name}: {} checks, {} mismatches", self.n, self.fails.len());
        if !self.fails.is_empty() {
            panic!(
                "{name}: {} mismatches:\n{}",
                self.fails.len(),
                self.fails.join("\n")
            );
        }
    }
}

#[test]
fn difflib_golden() {
    let mut c = Check::default();
    for case in load("difflib").as_array().unwrap() {
        let (a, b) = (case["a"].as_str().unwrap(), case["b"].as_str().unwrap());
        let sm = difflib::SequenceMatcher::new(a, b);
        let key = (
            a.chars().take(40).collect::<String>(),
            b.chars().take(40).collect::<String>(),
        );
        let r = sm.ratio();
        c.eq("ratio", &key, r.to_bits(), f(&case["ratio"]).to_bits());
        c.eq(
            "ratio repr",
            &key,
            pystr::float_repr(r),
            case["ratio"].as_str().unwrap().to_string(),
        );
        c.eq(
            "quick",
            &key,
            sm.quick_ratio().to_bits(),
            f(&case["quick"]).to_bits(),
        );
        c.eq(
            "real_quick",
            &key,
            sm.real_quick_ratio().to_bits(),
            f(&case["real_quick"]).to_bits(),
        );
        let blocks: Vec<Vec<usize>> = sm
            .get_matching_blocks()
            .iter()
            .map(|m| vec![m.a, m.b, m.size])
            .collect();
        let want: Vec<Vec<usize>> = serde_json::from_value(case["blocks"].clone()).unwrap();
        c.eq("blocks", &key, blocks, want);
        c.eq(
            "edit_pct",
            &key,
            difflib::edit_pct(a, b).to_string(),
            case["edit_pct"].as_str().unwrap().to_string(),
        );
        c.eq(
            "text_similarity",
            &key,
            matching::text_similarity(Some(a), Some(b)).to_bits(),
            f(&case["text_similarity"]).to_bits(),
        );
    }
    c.done("difflib");
}

#[test]
fn utils_golden() {
    let g = load("utils");
    let mut c = Check::default();
    for case in g["normalize_phone"].as_array().unwrap() {
        c.eq(
            "normalize_phone",
            &case["in"],
            utils::normalize_phone(s(&case["in"])),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in g["escape_like"].as_array().unwrap() {
        c.eq(
            "escape_like",
            &case["in"],
            utils::escape_like(s(&case["in"]).unwrap()),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in g["parse_uuid"].as_array().unwrap() {
        let got = utils::parse_uuid(s(&case["in"]).unwrap(), s(&case["field"]).unwrap())
            .map(|u| u.to_string());
        let want = match case.get("ok") {
            Some(ok) => Ok(ok.as_str().unwrap().to_string()),
            None => {
                assert_eq!(case["status"], 400);
                Err(case["err"].as_str().unwrap().to_string())
            }
        };
        c.eq("parse_uuid", &case["in"], got, want);
    }
    for case in g["client_ip"].as_array().unwrap() {
        c.eq(
            "client_ip",
            (&case["header"], &case["host"]),
            utils::get_client_ip(s(&case["header"]), s(&case["host"])),
            opt_string(&case["out"]),
        );
    }
    for case in g["add_business_days"].as_array().unwrap() {
        let start = s(&case["start"]).unwrap();
        let days = case["days"].as_i64().unwrap();
        let want = s(&case["out"]).unwrap().to_string();
        let got = if start.len() == 10 {
            utils::add_business_days(NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap(), days)
                .to_string()
        } else if start.ends_with("+00:00") {
            let d: DateTime<Utc> = DateTime::parse_from_rfc3339(start)
                .unwrap()
                .with_timezone(&Utc);
            utils::add_business_days(d, days)
                .format("%Y-%m-%dT%H:%M:%S+00:00")
                .to_string()
        } else {
            let d = NaiveDateTime::parse_from_str(start, "%Y-%m-%dT%H:%M:%S").unwrap();
            utils::add_business_days(d, days)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        };
        c.eq("add_business_days", (start, days), got, want);
    }
    c.done("utils");
}

#[test]
fn relativedelta_golden() {
    let mut c = Check::default();
    for case in load("relativedelta").as_array().unwrap() {
        let start = s(&case["start"]).unwrap();
        let (y, m, d) = (
            case["years"].as_i64().unwrap(),
            case["months"].as_i64().unwrap(),
            case["days"].as_i64().unwrap(),
        );
        let want = opt_string(&case["out"]);
        let got = if case["kind"] == "date" {
            relativedelta::add_date(
                NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap(),
                y,
                m,
                d,
            )
            .map(|x| x.format("%Y-%m-%d").to_string())
        } else if start.ends_with("+00:00") {
            let dt: DateTime<Utc> = DateTime::parse_from_rfc3339(start)
                .unwrap()
                .with_timezone(&Utc);
            relativedelta::add_utc(dt, y, m, d).map(|x| {
                if start.contains('.') {
                    x.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
                } else {
                    x.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
                }
            })
        } else {
            let dt = NaiveDateTime::parse_from_str(start, "%Y-%m-%dT%H:%M:%S").unwrap();
            relativedelta::add_naive(dt, y, m, d).map(|x| x.format("%Y-%m-%dT%H:%M:%S").to_string())
        };
        c.eq("relativedelta", (start, y, m, d), got, want);
    }
    // The helpers.
    let d = NaiveDate::from_ymd_opt(2024, 1, 31).unwrap();
    c.eq(
        "add_months",
        "2024-01-31 +1",
        relativedelta::add_months(d, 1),
        NaiveDate::from_ymd_opt(2024, 2, 29),
    );
    c.eq(
        "add_years",
        "2024-02-29 +1",
        relativedelta::add_years(NaiveDate::from_ymd_opt(2024, 2, 29).unwrap(), 1),
        NaiveDate::from_ymd_opt(2025, 2, 28),
    );
    c.done("relativedelta");
}

#[test]
fn regex_golden() {
    let g = load("regex");
    let mut c = Check::default();
    for case in g["fixed_temperature"].as_array().unwrap() {
        c.eq(
            "fixed_temperature",
            &case["in"],
            patterns::fixed_temperature(s(&case["in"])),
            case["out"].as_bool().unwrap(),
        );
    }
    for case in g["chunk_text"].as_array().unwrap() {
        c.eq(
            "chunk_text",
            &case["in"],
            patterns::chunk_text(s(&case["in"]).unwrap()),
            strs(&case["out"]),
        );
    }
    for case in g["sentence_split"].as_array().unwrap() {
        c.eq(
            "sentence_split",
            &case["in"],
            patterns::py_split(&patterns::SENTENCE_SPLIT, s(&case["in"]).unwrap()),
            strs(&case["out"]),
        );
    }
    for case in g["guide_area_split"].as_array().unwrap() {
        c.eq(
            "guide_area_split",
            &case["in"],
            patterns::guide_area_split(s(&case["in"]).unwrap()),
            strs(&case["out"]),
        );
    }
    for case in g["style_section"].as_array().unwrap() {
        c.eq(
            "style_section",
            (&case["in"], &case["channel"]),
            patterns::style_section(s(&case["in"]), s(&case["channel"])),
            case["out"].as_str().unwrap().to_string(),
        );
    }
    c.done("regex");
}

#[test]
fn strs_golden() {
    let g = load("strs");
    let mut c = Check::default();
    let arr = |k: &str| g[k].as_array().unwrap().clone();
    for case in arr("split") {
        let i = s(&case["in"]).unwrap();
        c.eq(
            "split",
            i,
            pystr::split(i).iter().map(|x| x.to_string()).collect(),
            strs(&case["out"]),
        );
    }
    for case in arr("strip") {
        let i = s(&case["in"]).unwrap();
        c.eq(
            "strip",
            i,
            pystr::strip(i).to_string(),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("lower") {
        let i = s(&case["in"]).unwrap();
        c.eq(
            "lower",
            i,
            pystr::lower(i),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("title") {
        let i = s(&case["in"]).unwrap();
        c.eq(
            "title",
            i,
            pystr::title(i),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("splitlines") {
        let i = s(&case["in"]).unwrap();
        c.eq(
            "splitlines",
            i,
            pystr::splitlines(i).iter().map(|x| x.to_string()).collect(),
            strs(&case["out"]),
        );
    }
    for case in arr("isdigit_join") {
        let i = s(&case["in"]).unwrap();
        c.eq(
            "isdigit_join",
            i,
            pystr::digit_chars(i),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("strip_chars") {
        let i = s(&case["in"]).unwrap();
        c.eq(
            "strip_chars",
            i,
            pystr::strip_chars(i, s(&case["chars"]).unwrap()).to_string(),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("normalize_name") {
        c.eq(
            "normalize_name",
            &case["in"],
            matching::normalize_name(s(&case["in"])),
            case["out"].as_str().unwrap().into(),
        );
    }
    let honor = [
        "Mr", "Ms", "Mr", "Ms", "Mr.", "Mrs.", "Dr.", "Ir.", "H.", "Hj.",
    ];
    for case in arr("clean_person") {
        c.eq(
            "clean_person",
            &case["in"],
            matching::clean_person(s(&case["in"]), &honor),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("research_normalise_name") {
        c.eq(
            "research_normalise_name",
            &case["in"],
            matching::research_normalise_name(s(&case["in"])),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("jaccard") {
        let (a, b) = (s(&case["a"]).unwrap(), s(&case["b"]).unwrap());
        c.eq(
            "jaccard",
            (a, b),
            matching::jaccard(a, b).to_bits(),
            f(&case["dedupe"]).to_bits(),
        );
        c.eq(
            "jaccard identity",
            (a, b),
            matching::jaccard(a, b).to_bits(),
            f(&case["identity"]).to_bits(),
        );
    }
    for case in arr("text_similarity") {
        c.eq(
            "text_similarity",
            (&case["a"], &case["b"]),
            matching::text_similarity(s(&case["a"]), s(&case["b"])).to_bits(),
            f(&case["out"]).to_bits(),
        );
    }
    // float_repr layout spot checks (Python repr).
    for (x, want) in [
        (1.0, "1.0"),
        (0.5, "0.5"),
        (1e-05, "1e-05"),
        (0.0001, "0.0001"),
        (1e16, "1e+16"),
        (123456789012345.6, "123456789012345.6"),
        (0.0, "0.0"),
        (2.5e-7, "2.5e-07"),
    ] {
        c.eq("float_repr", x, pystr::float_repr(x), want.to_string());
    }
    c.done("strs");
}

/// Library verdicts (validity, formatting, regions) come from the `phonenumber` crate run on
/// libphonenumber 8.13.51 metadata, the release Python phonenumbers 8.13.51 uses: every case must match.
#[test]
fn phones_golden() {
    let g = load("phones");
    let mut c = Check::default();
    let mut lib = Check::default();
    let arr = |k: &str| g[k].as_array().unwrap().clone();
    for case in arr("clean_name") {
        c.eq(
            "clean_name",
            &case["in"],
            phones::clean_name(s(&case["in"])),
            opt_string(&case["out"]),
        );
    }
    for case in arr("digits") {
        c.eq(
            "digits",
            &case["in"],
            phones::digits(s(&case["in"])),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("chat_id_for") {
        c.eq(
            "chat_id_for",
            &case["in"],
            phones::chat_id_for(s(&case["in"]).unwrap()),
            case["out"].as_str().unwrap().into(),
        );
    }
    for case in arr("country_of_timezone") {
        c.eq(
            "country_of_timezone",
            &case["in"],
            phones::country_of_timezone(s(&case["in"])),
            opt_string(&case["out"]),
        );
    }
    for case in arr("to_e164") {
        let country = s(&case["country"]).unwrap();
        lib.eq(
            "to_e164",
            (&case["in"], country),
            phones::to_e164(s(&case["in"]), country),
            opt_string(&case["out"]),
        );
    }
    for case in arr("from_display_name") {
        let country = s(&case["country"]).unwrap();
        lib.eq(
            "from_display_name",
            (&case["in"], country),
            phones::from_display_name(s(&case["in"]), country),
            opt_string(&case["out"]),
        );
    }
    for case in arr("pretty") {
        lib.eq(
            "pretty",
            &case["in"],
            phones::pretty(s(&case["in"])),
            opt_string(&case["out"]),
        );
    }
    for case in arr("country_of") {
        lib.eq(
            "country_of",
            &case["in"],
            phones::country_of(s(&case["in"])),
            opt_string(&case["out"]),
        );
    }
    for case in arr("phones_in_text") {
        lib.eq(
            "phones_in_text",
            &case["in"],
            phones::phones_in_text(s(&case["in"]), "JP"),
            strs(&case["out"]),
        );
    }
    eprintln!(
        "phones library-backed: {} checks, {} differ",
        lib.n,
        lib.fails.len()
    );
    for f in &lib.fails {
        eprintln!("  {f}");
    }
    c.done("phones");
    assert!(
        lib.fails.is_empty(),
        "phonenumber verdicts differ from Python: {}",
        lib.fails.len()
    );
}

/// The phonenumbers port itself: parse (country code, national significant number, extension, or the
/// error kind), is_possible_number, is_valid_number, region_code_for_number, E164 and INTERNATIONAL.
#[test]
fn phonelib_golden() {
    let mut c = Check::default();
    for case in load("phonelib").as_array().unwrap() {
        let raw = s(&case["in"]).unwrap();
        let region = s(&case["region"]);
        let key = (raw, region);
        match phonelib::parse(raw, region) {
            Err(e) => c.eq(
                "parse",
                key,
                Some(format!("{e:?}")),
                opt_string(&case["err"]),
            ),
            Ok(n) => {
                c.eq("parse", key, None, opt_string(&case["err"]));
                if case.get("err").is_some() {
                    continue;
                }
                c.eq(
                    "cc",
                    key,
                    n.country_code as u64,
                    case["cc"].as_u64().unwrap(),
                );
                c.eq(
                    "nsn",
                    key,
                    n.nsn.clone(),
                    case["nsn"].as_str().unwrap().to_string(),
                );
                c.eq("ext", key, n.extension.clone(), opt_string(&case["ext"]));
                c.eq(
                    "possible",
                    key,
                    phonelib::is_possible_number(&n),
                    case["possible"].as_bool().unwrap(),
                );
                c.eq(
                    "valid",
                    key,
                    phonelib::is_valid_number(&n),
                    case["valid"].as_bool().unwrap(),
                );
                c.eq(
                    "region_of",
                    key,
                    phonelib::region_code_for_number(&n),
                    opt_string(&case["region_of"]),
                );
                c.eq(
                    "e164",
                    key,
                    phonelib::format_e164(&n),
                    case["e164"].as_str().unwrap().to_string(),
                );
                c.eq(
                    "intl",
                    key,
                    phonelib::format_international(&n),
                    case["intl"].as_str().unwrap().to_string(),
                );
            }
        }
    }
    c.done("phonelib");
}
