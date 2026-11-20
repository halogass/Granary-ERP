//! Golden replay: email.rs / email/compat32.rs against what the real Python `_send_via_config` hands to
//! aiosmtplib (compat-tests/golden/gen_email.py): header folding (compat32 `Header.encode`), the whole
//! serialised message (headers, multipart nesting, inline images, attachments, encodings, boundaries in
//! Python's order), the Sent-folder capture, the envelope addresses and the Message-ID domain.
//! The outputs re-record with `GOLDEN_BLESS=1` (tests/golden/bless.rs): the Rust code is now the reference.

#[path = "golden/bless.rs"]
mod bless;

use base64::Engine as _;
use granary_services::email::{self, compat32, Attachment, SmtpConfig};
use granary_services::email_signature::InlinePart;
use serde_json::{json, Value};

fn load(name: &str) -> Vec<Value> {
    serde_json::from_value(bless::read(&format!("email/{name}.json"))).expect("fixture JSON")
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

const DATE: &str = "Wed, 14 Oct 2026 10:11:12 +0900";

#[test]
fn header_folding_is_pythons() {
    let mut cases = load("fold");
    let mut bad = 0;
    for c in cases.iter_mut() {
        let got = compat32::fold(
            c["name"].as_str().unwrap(),
            c["value"].as_str().unwrap(),
            "\r\n",
        );
        if bless::blessing() {
            let o = c.as_object_mut().unwrap();
            match &got {
                Ok(g) => o.insert("out".into(), json!(g)),
                Err(_) => o.remove("out"),
            };
            continue;
        }
        let ok = match (&got, c.get("out")) {
            (Ok(g), Some(w)) => g == w.as_str().unwrap(),
            (Err(_), None) => true,
            _ => false,
        };
        if !ok {
            bad += 1;
            if bad < 6 {
                eprintln!(
                    "{}: {:?}\n want {:?}\n  got {:?}",
                    c["name"],
                    c["value"],
                    c.get("out"),
                    got
                );
            }
        }
    }
    assert_eq!(bad, 0, "{bad} of {} differ", cases.len());
    bless::save("email/fold.json", &Value::Array(cases));
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

pub fn cfg_of(v: &Value) -> SmtpConfig {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    SmtpConfig {
        host: s("host"),
        port: v.get("port").and_then(|x| x.as_i64()).unwrap_or(587),
        user: s("user"),
        password: s("password"),
        from_name: s("from_name"),
        from_email: s("from_email"),
        tls: v.get("tls").and_then(|x| x.as_bool()).unwrap_or(false),
    }
}

#[test]
fn messages_are_byte_identical_to_python() {
    let mut cases = load("messages");
    let mut msgid_n = 0;
    for (i, slot) in cases.iter_mut().enumerate() {
        let c = &slot.clone();
        if c.get("not_configured").is_some() {
            continue;
        }
        let cfg = cfg_of(&c["cfg"]);
        let to = if c["to"].is_string() {
            vec![c["to"].as_str().unwrap().to_string()]
        } else {
            strs(&c["to"])
        };
        let cc = c["cc"].as_array().map(|_| strs(&c["cc"]));
        let atts: Option<Vec<Attachment>> = c["attachments"].as_array().map(|a| {
            a.iter()
                .map(|x| Attachment {
                    filename: x["filename"].as_str().unwrap().into(),
                    content: b64(x["content"].as_str().unwrap()),
                    mime_type: x["mime_type"].as_str().map(|s| s.into()),
                })
                .collect()
        });
        let parts: Vec<InlinePart> = c["inline_parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| InlinePart {
                cid: p["cid"].as_str().unwrap().into(),
                data: b64(p["data"].as_str().unwrap()),
                mime: p["mime"].as_str().unwrap().into(),
                filename: p["filename"].as_str().unwrap().into(),
            })
            .collect();
        let headers: Option<Vec<(String, String)>> = c["headers"].as_array().map(|h| {
            h.iter()
                .map(|kv| {
                    (
                        kv[0].as_str().unwrap().into(),
                        kv[1].as_str().unwrap().into(),
                    )
                })
                .collect()
        });
        let mut domain_seen: Option<Value> = None;
        let mut msg = email::compose(
            &cfg,
            &to,
            c["subject"].as_str().unwrap(),
            c["html"].as_str().unwrap(),
            cc.as_deref(),
            atts.as_deref(),
            &parts,
            headers.as_deref(),
            DATE,
            &mut |d| {
                msgid_n += 1;
                domain_seen = Some(json!(d));
                format!("<msgid.{msgid_n}@{}>", d.unwrap_or("None"))
            },
        );
        bless::eq(
            &mut slot["msgid_domain"],
            domain_seen.unwrap_or(json!("<not called>")),
            format!("case {i} msgid domain"),
        );
        let mut n = 0u64;
        let mut boundary = || {
            n += 1;
            format!("==============={n:019}==")
        };
        let wire = compat32::flatten(&mut msg, "\r\n", &mut boundary, false).unwrap();
        bless::eq(&mut slot["wire"], wire, format!("case {i} wire"));
        let raw = compat32::flatten(&mut msg, "\n", &mut boundary, true).unwrap();
        bless::eq(&mut slot["capture_raw"], raw, format!("case {i} capture"));
        bless::eq(
            &mut slot["capture_message_id"],
            msg.get("Message-ID").unwrap(),
            format!("case {i} message id"),
        );
        let (from, rcpt) = email::envelope_of(&msg);
        bless::eq(&mut slot["sender"], json!(from), format!("case {i} sender"));
        bless::eq(
            &mut slot["recipients"],
            json!(rcpt),
            format!("case {i} recipients"),
        );
        assert_eq!(c["kw"]["start_tls"], json!(cfg.tls));
    }
    bless::save("email/messages.json", &Value::Array(cases));
}

#[tokio::test]
async fn an_unconfigured_account_sends_nothing() {
    for c in load("messages")
        .iter()
        .filter(|c| c.get("not_configured").is_some())
    {
        let cfg = cfg_of(&c["cfg"]);
        let ok = email::_send_via_config(
            &cfg,
            &["x@y".to_string()],
            "s",
            "<p>",
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(json!(ok), c["ok"]);
    }
}
