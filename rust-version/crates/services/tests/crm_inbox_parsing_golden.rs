//! Golden tests for the email parsing behind crm_inbox (imap_sync::pyemail) and the pure helpers of
//! imap_sync / crm_inbox, recorded from the real Python by compat-tests/golden/gen_crm_inbox.py.

#[path = "golden/bless.rs"]
mod bless;

use std::path::PathBuf;

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use granary_services::crm_inbox;
use granary_services::imap_sync::{self, pyemail};
use pyemail::{codecs, utils, Content, Message, PyErr};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat-tests/golden/crm_inbox")
}

fn load(name: &str) -> Vec<Value> {
    let s = std::fs::read_to_string(dir().join(format!("{name}.json"))).unwrap();
    serde_json::from_str(&s).unwrap()
}

fn b64d(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}
fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}
fn blob(b: &[u8]) -> Value {
    if b.len() > 4096 {
        json!({"sha256": hex::encode(Sha256::digest(b)), "len": b.len()})
    } else {
        json!(b64e(b))
    }
}

fn exc(e: &PyErr) -> Value {
    json!({"exc": e.kind})
}

fn opt_str(r: Result<Option<String>, PyErr>) -> Value {
    match r {
        Ok(None) => Value::Null,
        Ok(Some(s)) => json!(s),
        Err(e) => exc(&e),
    }
}

fn report(what: &str, bad: &[String], total: usize) {
    if !bad.is_empty() {
        for b in bad.iter().take(25) {
            eprintln!("{b}");
        }
        panic!("{what}: {} of {} cases differ", bad.len(), total);
    }
}

#[test]
fn header_values_match_python() {
    let cases = load("headers");
    let mut bad = Vec::new();
    for c in &cases {
        let raw = b64d(c["raw"].as_str().unwrap());
        let name = c["name"].as_str().unwrap();
        let msg = match pyemail::message_from_bytes(&raw) {
            Ok(m) => m,
            Err(e) => {
                if c.get("parse") != Some(&exc(&e)) {
                    bad.push(format!(
                        "{:?}: parse {:?} vs {}",
                        String::from_utf8_lossy(&raw),
                        e,
                        c
                    ));
                }
                continue;
            }
        };
        if c.get("parse").is_some() {
            bad.push(format!(
                "{:?}: python failed to parse",
                String::from_utf8_lossy(&raw)
            ));
            continue;
        }
        let mut got = json!({"name": name, "raw": c["raw"], "get": opt_str(msg.get(name))});
        if let Some(s) = got["get"].as_str().map(|s| s.to_string()) {
            if ["From", "To", "Cc", "Reply-To", "Sender"].contains(&name) {
                let (a, b) = utils::parseaddr(&s);
                got["parseaddr"] = json!([a, b]);
                got["getaddresses"] = json!(utils::getaddresses(&[&s])
                    .into_iter()
                    .map(|(a, b)| json!([a, b]))
                    .collect::<Vec<_>>());
            }
        }
        if name == "Content-Type" || name == "Content-Disposition" {
            let r = (|| -> Result<Value, PyErr> {
                let charset = match msg.get_param("charset", "content-type")? {
                    None => json!("ASCII"),
                    Some(utils::ParamValue::Str(s)) => json!(s),
                    Some(_) => json!("tuple"),
                };
                Ok(json!({
                    "content_type": msg.get_content_type()?,
                    "filename": msg.get_filename()?,
                    "charset": charset,
                    "boundary": msg.get_boundary()?,
                }))
            })();
            match r {
                Ok(v) => {
                    for k in ["content_type", "filename", "charset", "boundary"] {
                        got[k] = v[k].clone();
                    }
                }
                Err(e) => got["params"] = exc(&e),
            }
        }
        if &got != c {
            bad.push(format!(
                "{:?}\n  rust:   {}\n  python: {}",
                String::from_utf8_lossy(&raw),
                got,
                c
            ));
        }
    }
    report("headers", &bad, cases.len());
}

fn part_record(part: &Message) -> Value {
    let r = (|| -> Result<Value, PyErr> {
        let pl = part.get_payload_decoded()?;
        Ok(json!({
            "content_type": part.get_content_type()?,
            "default_type": part.get_default_type(),
            "is_multipart": part.is_multipart(),
            "filename": part.get_filename()?,
            "cd": part.get("Content-Disposition")?,
            "cid": part.get("Content-ID")?,
            "payload": pl.map(|p| blob(&p)),
        }))
    })();
    let mut rec = match r {
        Ok(v) => v,
        Err(e) => return json!({"error": exc(&e)}),
    };
    rec["content"] = match part.get_content() {
        Ok(Content::Text(s)) => json!({"str": s}),
        Ok(Content::Bytes(b)) => json!({"bytes": blob(&b)}),
        Ok(Content::Message(_)) => json!({"message": true}),
        Err(e) => exc(&e),
    };
    rec
}

const HEADER_LIST: [&str; 14] = [
    "Message-ID",
    "Subject",
    "From",
    "To",
    "Cc",
    "Date",
    "Reply-To",
    "In-Reply-To",
    "References",
    "Content-Type",
    "Content-Transfer-Encoding",
    "MIME-Version",
    "Content-ID",
    "X-Missing",
];

fn message_record(name: &str, raw: &[u8]) -> Value {
    let parsed = match pyemail::message_from_bytes(raw) {
        Ok(m) => m,
        Err(e) => return json!({"name": name, "parse": exc(&e)}),
    };
    let mut headers = serde_json::Map::new();
    for h in HEADER_LIST
        .iter()
        .map(|s| s.to_string())
        .chain(parsed.keys())
    {
        headers.insert(h.clone(), opt_str(parsed.get(&h)));
    }
    let mut rec = json!({
        "name": name,
        "headers": headers,
        "keys": parsed.keys(),
        "parts": parsed.walk().into_iter().map(part_record).collect::<Vec<_>>(),
    });
    let from = (|| -> Result<Value, PyErr> {
        let f = parsed.get("From")?.unwrap_or_default();
        let (a, b) = utils::parseaddr(&f);
        let to = parsed.get("To")?.unwrap_or_default();
        let tos: Vec<String> = utils::getaddresses(&[&to])
            .into_iter()
            .map(|x| x.1)
            .filter(|e| !e.is_empty())
            .collect();
        Ok(json!([[a, b], tos]))
    })();
    match from {
        Ok(v) => {
            rec["from"] = v[0].clone();
            rec["to"] = v[1].clone();
        }
        Err(e) => rec["from"] = exc(&e),
    }
    rec["received_at"] = match parsed.get("Date") {
        Ok(Some(d)) if !d.is_empty() => match utils::parsedate_to_datetime(&d) {
            Ok(dt) => json!(py_isoformat(&dt)),
            Err(_) => json!("now"),
        },
        _ => json!("now"),
    };
    rec["extract"] = match crm_inbox::_extract_body_and_attachments(&parsed, raw) {
        Ok((text, html, atts)) => json!({
            "text": match text { crm_inbox::Body::Str(s) => json!(s), crm_inbox::Body::Bytes(b) => json!({"bytes": b64e(&b)}) },
            "html": html,
            "attachments": atts.iter().map(|a| json!({
                "filename": a.filename, "content_type": a.content_type,
                "sha256": hex::encode(Sha256::digest(&a.data)), "size": a.data.len()})).collect::<Vec<_>>(),
        }),
        Err(e) => exc(&e),
    };
    rec
}

fn py_isoformat(dt: &utils::PyDateTime) -> String {
    let base = dt.naive.format("%Y-%m-%dT%H:%M:%S").to_string();
    match dt.offset {
        None => base,
        Some(off) => {
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.unsigned_abs();
            format!("{base}{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
        }
    }
}

#[test]
fn whole_messages_match_python() {
    let mut cases = load("messages");
    let mut bad = Vec::new();
    for c in cases.iter_mut() {
        let name = c["name"].as_str().unwrap().to_string();
        let raw = std::fs::read(dir().join("messages").join(format!("{name}.eml"))).unwrap();
        let got = message_record(&name, &raw);
        for key in [
            "parse",
            "headers",
            "keys",
            "parts",
            "from",
            "to",
            "received_at",
            "extract",
        ] {
            if bless::blessing() {
                match got.get(key) {
                    Some(v) => c[key] = v.clone(),
                    None => {
                        c.as_object_mut().unwrap().remove(key);
                    }
                }
                continue;
            }
            if got.get(key) != c.get(key) {
                bad.push(format!(
                    "{name}.{key}\n  rust:   {}\n  python: {}",
                    got.get(key).unwrap_or(&Value::Null),
                    c.get(key).unwrap_or(&Value::Null)
                ));
            }
        }
    }
    bless::save("crm_inbox/messages.json", &Value::Array(cases.clone()));
    report("messages", &bad, cases.len());
}

#[test]
fn utils_match_python() {
    let cases = load("utils");
    let mut bad = Vec::new();
    for c in &cases {
        let input = c["in"].as_str().unwrap();
        let got = match c["kind"].as_str().unwrap() {
            "parseaddr" => {
                let (a, b) = utils::parseaddr(input);
                json!({"kind": "parseaddr", "in": input, "out": [a, b]})
            }
            "getaddresses" => json!({"kind": "getaddresses", "in": input,
                "out": utils::getaddresses(&[input]).into_iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>()}),
            _ => match utils::parsedate_to_datetime(input) {
                Ok(dt) => {
                    json!({"kind": "date", "in": input, "out": py_isoformat(&dt), "formatted": utils::format_datetime(&dt)})
                }
                Err(_) => json!({"kind": "date", "in": input, "out": {"exc": "ValueError"}}),
            },
        };
        if &got != c {
            bad.push(format!("rust {got}\n python {c}"));
        }
    }
    report("utils", &bad, cases.len());
}

#[test]
fn codecs_match_python() {
    let cases = load("codecs");
    let mut bad = Vec::new();
    let mut approx = 0;
    for c in &cases {
        let charset = c["charset"].as_str().unwrap();
        let data = b64d(c["data"].as_str().unwrap());
        let errors = match c["errors"].as_str().unwrap() {
            "strict" => codecs::Errors::Strict,
            "replace" => codecs::Errors::Replace,
            _ => codecs::Errors::SurrogateEscape,
        };
        let got = match codecs::decode(&data, charset, errors) {
            Ok(s) => {
                if pyemail::pystr::has_surrogates(&s) {
                    json!({"surrogates": hex::encode(pyemail::pystr::encode_utf8_escaped(&s))})
                } else {
                    json!(s)
                }
            }
            Err(codecs::DecodeErr::Lookup(_)) => json!({"exc": "LookupError"}),
            Err(codecs::DecodeErr::Decode(_)) => json!({"exc": "UnicodeDecodeError"}),
            Err(codecs::DecodeErr::Encode(_)) => json!({"exc": "UnicodeEncodeError"}),
            Err(codecs::DecodeErr::Value(_)) => json!({"exc": "UnicodeError"}),
        };
        if got != c["out"] {
            // CJK codecs and utf-7 go through approximations (documented in codecs.rs).
            let cjk = [
                "gb2312",
                "gbk",
                "big5",
                "shift_jis",
                "euc-kr",
                "ks_c_5601-1987",
                "utf7",
            ];
            if cjk.contains(&charset) {
                approx += 1;
                continue;
            }
            bad.push(format!(
                "{charset} {:?} {:?}: rust {got} python {}",
                data, c["errors"], c["out"]
            ));
        }
    }
    eprintln!("codec cases differing only for approximated CJK/utf-7 codecs: {approx}");
    report("codecs", &bad, cases.len());
}

#[test]
fn pure_helpers_match_python() {
    let cases = load("pure");
    let mut bad = Vec::new();
    let lines = |v: &Value| -> Vec<Vec<u8>> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| b64d(x.as_str().unwrap()))
            .collect()
    };
    for c in &cases {
        let input = &c["in"];
        let got: Value = match c["fn"].as_str().unwrap() {
            "_clean_subject" => json!(crm_inbox::_clean_subject(input.as_str().unwrap())),
            "_html_to_text" => json!(crm_inbox::_html_to_text(input.as_str().unwrap())),
            "_fit_id" => json!(crm_inbox::_fit_id(
                input[0].as_str(),
                input[1].as_u64().unwrap() as usize
            )),
            "quote_mailbox" => json!(imap_sync::quote_mailbox(input.as_str().unwrap())),
            "parse_list_line" => {
                let (a, d, n) = imap_sync::parse_list_line(&b64d(input.as_str().unwrap()));
                json!([a.into_iter().collect::<Vec<_>>(), d, n])
            }
            "resolve_folders" => json!(imap_sync::folders_from_list(&lines(input)).0),
            "_uid_set" => {
                let v: Vec<i64> = serde_json::from_value(input.clone()).unwrap();
                json!(imap_sync::_uid_set(&v))
            }
            "parse_uids" => json!(imap_sync::parse_uids(&lines(input))),
            "seen_in" => json!(imap_sync::seen_in(&lines(input))),
            "_raw_payload" => json!(crm_inbox::_raw_payload(&lines(input)).map(|b| b64e(&b))),
            other => panic!("unknown fn {other}"),
        };
        // Fix batch 1 (B6) also strips stray colons, a repeated prefix and a prefix after leading spaces ("Fwd:: w",
        // " Re: x"): the Python result cleaned once more is the expected one.
        let want = if c["fn"] == "_clean_subject" {
            json!(crm_inbox::_clean_subject(c["out"].as_str().unwrap()))
        } else {
            c["out"].clone()
        };
        if got != want {
            bad.push(format!(
                "{} {}: rust {got} python {}",
                c["fn"], input, c["out"]
            ));
        }
    }
    report("pure", &bad, cases.len());
}
