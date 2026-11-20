//! Golden replay of nary/{briefs,report,owner_channel,rhythm}.py's pure parts (fixtures from
//! compat-tests/golden/gen_nary_owner_rhythm.py, recorded from the real Python functions). working_time re-records
//! with GOLDEN_BLESS=1 (Rust is now the reference; review the diff).

#[path = "golden/bless.rs"]
mod bless;

use chrono::{DateTime, Utc};
use granary_compat::pyjson::PyDateTime;
use granary_core::errors::ApiError;
use granary_db::rows::nary::NaryOwnerTopic;
use granary_services::nary::{briefs, owner_channel, report, rhythm};
use serde_json::Value;
use uuid::Uuid;

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../compat-tests/golden/nary_owner_rhythm/{name}.json"
    ));
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn ts(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .map(|s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
}

#[test]
fn briefs_render_plain_and_rest() {
    let cases = fixture("briefs_render");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 600);
    for (i, c) in cases.iter().enumerate() {
        let cap = c["cap"].as_i64();
        assert_eq!(
            briefs::render_plain(&c["data"], s(&c["slot"]), cap),
            s(&c["plain"]),
            "case {i}"
        );
        if let Some(cap) = cap {
            assert_eq!(
                briefs::render_rest(&c["data"], s(&c["slot"]), cap),
                s(&c["rest"]),
                "rest case {i}"
            );
        }
    }
}

#[test]
fn report_render_plain() {
    let cases = fixture("report_render");
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        assert_eq!(report::render_plain(&c["data"]), s(&c["plain"]), "case {i}");
    }
}

#[test]
fn loss_review_text() {
    let cases = fixture("loss_review");
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        let data = &c["data"];
        let plain = report::loss_plain(data);
        let rows = c["rows"].as_array().unwrap();
        let body = if rows.is_empty() {
            plain
        } else {
            format!("MODEL:{plain}")
        };
        assert_eq!(body, s(&c["body"]), "case {i}");
        let month = s(&data["month"]);
        assert_eq!(format!("Loss review {month}"), s(&c["title"]));
        assert_eq!(format!("loss:{month}"), s(&c["entity_id"]));
    }
}

#[test]
fn loss_review_previous_month() {
    use granary_compat::pyjson::PyTz;
    for (y, m, want) in [
        (2026, 1, "2025-12"),
        (2026, 3, "2026-02"),
        (2027, 12, "2027-11"),
    ] {
        let start = PyDateTime {
            local: chrono::NaiveDate::from_ymd_opt(y, m, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            tz: Some(PyTz::Zone(chrono_tz::Asia::Tokyo)),
        };
        assert_eq!(
            report::prev_month_start(&start)
                .local
                .format("%Y-%m")
                .to_string(),
            want
        );
    }
}

#[test]
fn compose_messages() {
    let cases = fixture("compose");
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        let topics: Vec<NaryOwnerTopic> = c["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                let mut row = blank_topic();
                row.tag = s(&t["tag"]).into();
                row.text = s(&t["text"]).into();
                row.urgent = t["urgent"].as_bool().unwrap();
                row.kind = s(&t["kind"]).into();
                row
            })
            .collect();
        let words = owner_channel::phrases(s(&c["lang"])).unwrap();
        assert_eq!(
            owner_channel::_compose(&topics, words, c["more"].as_i64().unwrap()),
            s(&c["text"]),
            "case {i}"
        );
    }
}

#[test]
fn detect_language() {
    let cases = fixture("detect_language");
    for c in cases.as_array().unwrap() {
        assert_eq!(
            owner_channel::detect_language(c["text"].as_str()),
            c["lang"].as_str(),
            "{:?}",
            c["text"]
        );
    }
}

fn blank_topic() -> NaryOwnerTopic {
    NaryOwnerTopic {
        kind: String::new(),
        tag: String::new(),
        text: String::new(),
        ref_type: None,
        ref_id: None,
        facts: None,
        status: "queued".into(),
        urgent: false,
        anytime: false,
        numbered: None,
        resume: None,
        send_after: None,
        sent_at: None,
        wa_message_keys: None,
        conversation_id: None,
        remind_at: None,
        reminded_at: None,
        answered_at: None,
        answer: None,
        id: Uuid::nil(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn topic_of(r: &Value) -> NaryOwnerTopic {
    let opt = |v: &Value| if v.is_null() { None } else { Some(v.clone()) };
    NaryOwnerTopic {
        id: Uuid::parse_str(s(&r["id"])).unwrap(),
        kind: s(&r["kind"]).into(),
        tag: s(&r["tag"]).into(),
        text: s(&r["text"]).into(),
        status: s(&r["status"]).into(),
        ref_type: r["ref_type"].as_str().map(|x| x.into()),
        ref_id: r["ref_id"].as_str().map(|x| Uuid::parse_str(x).unwrap()),
        facts: opt(&r["facts"]),
        urgent: r["urgent"].as_bool().unwrap(),
        numbered: opt(&r["numbered"]),
        sent_at: ts(&r["sent_at"]),
        answered_at: ts(&r["answered_at"]),
        answer: r["answer"].as_str().map(|x| x.into()),
        ..blank_topic()
    }
}

#[test]
fn serialize_topics() {
    let cases = fixture("serialize");
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        let out = owner_channel::serialize(&topic_of(&c["row"]));
        assert_eq!(
            serde_json::to_string(&out).unwrap(),
            serde_json::to_string(&c["out"]).unwrap(),
            "case {i}"
        );
    }
}

#[test]
fn context_block() {
    let cases = fixture("context_block");
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        let open: Vec<NaryOwnerTopic> =
            c["open"].as_array().unwrap().iter().map(topic_of).collect();
        // context_block: the quoted topic first, then the open ones without it.
        let (topics, quoted) = match &c["quoted"] {
            Value::Null => (open, None),
            q => {
                let q = topic_of(q);
                let mut v = vec![q.clone()];
                v.extend(open.into_iter().filter(|t| t.id != q.id));
                (v, Some(q.id))
            }
        };
        assert_eq!(
            owner_channel::context_lines(&topics, quoted).unwrap(),
            s(&c["out"]),
            "case {i}"
        );
    }
}

fn err_detail(e: ApiError) -> String {
    match e {
        ApiError::Http { detail, .. } => detail.as_str().unwrap().to_string(),
        other => panic!("not an HTTP error: {other:?}"),
    }
}

#[test]
fn rhythm_parsers() {
    let f = fixture("rhythm_parse");
    for c in f["hhmm"].as_array().unwrap() {
        match rhythm::_hhmm(&c["value"], "nary.working_hours") {
            Ok(t) => assert_eq!(
                t.format("%H:%M").to_string(),
                s(&c["ok"]),
                "{:?}",
                c["value"]
            ),
            Err(e) => assert_eq!(e.detail, s(&c["error"]), "{:?}", c["value"]),
        }
    }
    for c in f["working_hours"].as_array().unwrap() {
        match rhythm::parse_working_hours(&c["value"]) {
            Ok(wh) => {
                assert_eq!(
                    serde_json::json!(wh.days),
                    c["ok"]["days"],
                    "{:?}",
                    c["value"]
                );
                assert_eq!(wh.start.format("%H:%M").to_string(), s(&c["ok"]["start"]));
                assert_eq!(wh.end.format("%H:%M").to_string(), s(&c["ok"]["end"]));
            }
            Err(e) => assert_eq!(err_detail(e), s(&c["error"]), "{:?}", c["value"]),
        }
    }
    for c in f["brief_slots"].as_array().unwrap() {
        match rhythm::parse_brief_slots(&c["value"]) {
            Ok(v) => {
                let got: Vec<Value> = v
                    .iter()
                    .map(|(k, t)| serde_json::json!([k, t.format("%H:%M").to_string()]))
                    .collect();
                assert_eq!(Value::Array(got), c["ok"], "{:?}", c["value"]);
            }
            Err(e) => assert_eq!(err_detail(e), s(&c["error"]), "{:?}", c["value"]),
        }
    }
}

#[test]
fn working_time() {
    let mut cases = fixture("working_time");
    if bless::blessing() {
        // Python's isoformat of a UTC datetime: microseconds only when there are some.
        let iso = |t: DateTime<Utc>| {
            if t.timestamp_subsec_micros() == 0 {
                t.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
            } else {
                t.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
            }
        };
        for c in cases.as_array_mut().unwrap() {
            let wh = rhythm::parse_working_hours(&c["wh"]).unwrap();
            let tz: chrono_tz::Tz = s(&c["zone"]).parse().unwrap();
            let at = ts(&c["at"]).unwrap();
            c["in_hours"] = Value::Bool(rhythm::in_hours(&at.with_timezone(&tz), &wh));
            c["next_working_start"] =
                Value::String(iso(owner_channel::next_working_start_in(at, &wh, tz)));
            let hours = c["hours"].as_i64().unwrap();
            c["add_working_hours"] =
                Value::String(iso(owner_channel::add_working_hours_in(at, hours, &wh, tz)));
        }
        bless::write("nary_owner_rhythm/working_time.json", &cases);
    }
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        let wh = rhythm::parse_working_hours(&c["wh"]).unwrap();
        let tz: chrono_tz::Tz = s(&c["zone"]).parse().unwrap();
        let at = ts(&c["at"]).unwrap();
        assert_eq!(
            rhythm::in_hours(&at.with_timezone(&tz), &wh),
            c["in_hours"].as_bool().unwrap(),
            "in_hours {i}"
        );
        let nws = owner_channel::next_working_start_in(at, &wh, tz);
        assert_eq!(
            nws,
            ts(&c["next_working_start"]).unwrap(),
            "next_working_start {i}"
        );
        let awh = owner_channel::add_working_hours_in(at, c["hours"].as_i64().unwrap(), &wh, tz);
        assert_eq!(
            awh,
            ts(&c["add_working_hours"]).unwrap(),
            "add_working_hours {i}"
        );
    }
}
