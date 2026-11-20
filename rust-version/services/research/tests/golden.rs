//! Replays tests/fixtures/golden.json (recorded from the reference Python implementation, Python 3.12.15;
//! the generator is no longer shipped): html_to_markdown,
//! clean_markdown, title_of, links, check_url, ip_is_public, urljoin, urlsplit, html.unescape and
//! RobotFileParser must give the same answers in Rust.

use granary_research::guard::{self, GuardError};
use granary_research::{clean, htmlparse, pyurl, robots};
use serde_json::{json, Value};

fn golden() -> Value {
    serde_json::from_str(include_str!("fixtures/golden.json")).expect("golden.json")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

#[test]
fn html_to_markdown_matches_python() {
    let g = golden();
    let mut failures = Vec::new();
    for case in g["html_to_markdown"].as_array().unwrap() {
        let (md, title) = clean::html_to_markdown(s(&case[0]));
        let want_title = case[2].as_str().map(str::to_string);
        if md != s(&case[1]) || title != want_title {
            failures.push(format!(
                "{:?}\n  python: {:?} {:?}\n  rust:   {:?} {:?}",
                s(&case[0]),
                s(&case[1]),
                want_title,
                md,
                title
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn clean_markdown_and_title_match_python() {
    let g = golden();
    let mut failures = Vec::new();
    for case in g["clean_markdown"].as_array().unwrap() {
        let got = clean::clean_markdown(s(&case[0]), case[1].as_u64().unwrap() as usize);
        if got != s(&case[2]) {
            failures.push(format!(
                "clean {:?} {}\n  python: {:?}\n  rust:   {:?}",
                s(&case[0]),
                case[1],
                s(&case[2]),
                got
            ));
        }
    }
    for case in g["title_of"].as_array().unwrap() {
        let got = clean::title_of(s(&case[0]));
        if got.as_deref() != case[1].as_str() {
            failures.push(format!(
                "title {:?}: python {:?} rust {:?}",
                s(&case[0]),
                case[1],
                got
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn links_match_python() {
    let g = golden();
    let mut failures = Vec::new();
    for case in g["links"].as_array().unwrap() {
        let got = match clean::links(s(&case[0]), s(&case[1])) {
            Ok(v) => json!({"ok": v}),
            Err(_) => json!({"error": "ValueError"}),
        };
        if got != case[2] {
            failures.push(format!(
                "{:?} @ {:?}\n  python: {}\n  rust:   {}",
                s(&case[0]),
                s(&case[1]),
                case[2],
                got
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn check_url_matches_python() {
    let g = golden();
    let mut failures = Vec::new();
    for case in g["check_url"].as_array().unwrap() {
        let got = match guard::check_url(s(&case[0])) {
            Ok((scheme, host, port)) => json!({"ok": [scheme, host, port]}),
            Err(GuardError::Blocked(r)) => json!({"blocked": r}),
            Err(GuardError::Internal(_)) => json!({"error": "ValueError"}),
        };
        let mut want = case[1].clone();
        if want.get("error").is_some() {
            want = json!({"error": "ValueError"}); // UnicodeError and ValueError both answer 500
        }
        if got != want {
            failures.push(format!(
                "{:?}: python {} rust {}",
                s(&case[0]),
                case[1],
                got
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ip_is_public_matches_python() {
    let g = golden();
    let mut failures = Vec::new();
    for case in g["ip_is_public"].as_array().unwrap() {
        let got = match guard::ip_is_public(s(&case[0])) {
            Some(b) => json!({"ok": b}),
            None => json!({"error": "ValueError"}),
        };
        if got != case[1] {
            failures.push(format!(
                "{:?}: python {} rust {}",
                s(&case[0]),
                case[1],
                got
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn urljoin_and_urlsplit_match_python() {
    let g = golden();
    let mut failures = Vec::new();
    for case in g["urljoin"].as_array().unwrap() {
        let got = match pyurl::urljoin(s(&case[0]), s(&case[1])) {
            Ok(v) => json!({"ok": v}),
            Err(_) => json!({"error": "ValueError"}),
        };
        if got != case[2] {
            failures.push(format!(
                "urljoin({:?}, {:?}): python {} rust {}",
                s(&case[0]),
                s(&case[1]),
                case[2],
                got
            ));
        }
    }
    for case in g["urlsplit"].as_array().unwrap() {
        let got = match pyurl::urlsplit(s(&case[0])) {
            Ok(p) => json!({"ok": [p.scheme, p.netloc, p.path, p.query, p.fragment]}),
            Err(_) => json!({"error": "ValueError"}),
        };
        if got != case[1] {
            failures.push(format!(
                "urlsplit({:?}): python {} rust {}",
                s(&case[0]),
                case[1],
                got
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn unescape_matches_python() {
    let g = golden();
    for case in g["unescape"].as_array().unwrap() {
        assert_eq!(
            htmlparse::unescape(s(&case[0])),
            s(&case[1]),
            "{:?}",
            s(&case[0])
        );
    }
}

#[test]
fn robots_match_python() {
    let g = golden();
    let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36 Granary-Research/1.0";
    let mut failures = Vec::new();
    for case in g["robots"].as_array().unwrap() {
        let text = s(&case[0]);
        let lines = granary_compat::text::pystr::splitlines(text);
        let parsed = robots::RobotFileParser::parse(&lines);
        match (&parsed, case[1].as_array()) {
            (Err(_), None) => {}
            (Ok(robots), Some(results)) => {
                for r in results {
                    let got = match robots.can_fetch(ua, s(&r[0])) {
                        Ok(b) => json!({"ok": b}),
                        Err(_) => json!({"error": "ValueError"}),
                    };
                    if got != r[1] {
                        failures.push(format!("{text:?} {}: python {} rust {}", r[0], r[1], got));
                    }
                }
            }
            _ => failures.push(format!(
                "{text:?}: python {} rust parse {:?}",
                case[1],
                parsed.is_ok()
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
