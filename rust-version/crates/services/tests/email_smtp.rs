//! email.rs end to end: a real SMTP session against an in-process SMTP sink, compared with the session
//! the real Python `_send_via_config` (aiosmtplib) held with the same sink (compat-tests/golden/gen_email.py,
//! smtp.json); config loading from encrypted settings; the monkeypatch seams the Python tests use.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use granary_services::email::{self, Attachment, Capture, Seams, SendCall, SmtpConfig};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn load(name: &str) -> Vec<Value> {
    serde_json::from_value(bless::read(&format!("email/{name}.json"))).unwrap()
}

/// The same sink as the Python generator: 8BITMIME, AUTH PLAIN LOGIN, SIZE; records commands and DATA.
async fn sink() -> (u16, Arc<Mutex<(Vec<String>, String)>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let rec = Arc::new(Mutex::new((Vec::new(), String::new())));
    let r2 = rec.clone();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = sock.into_split();
        let mut rd = BufReader::new(rd);
        wr.write_all(b"220 sink ESMTP\r\n").await.unwrap();
        let mut in_data = false;
        loop {
            let mut line = String::new();
            if rd.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            if in_data {
                if line == ".\r\n" {
                    in_data = false;
                    wr.write_all(b"250 2.0.0 queued\r\n").await.unwrap();
                } else {
                    r2.lock().unwrap().1.push_str(&line);
                }
                continue;
            }
            let cmd = line.trim_end_matches("\r\n").to_string();
            r2.lock().unwrap().0.push(cmd.clone());
            let up = cmd.to_uppercase();
            let reply: &[u8] = if up.starts_with("EHLO") {
                b"250-sink\r\n250-8BITMIME\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 10000000\r\n"
            } else if up.starts_with("AUTH") {
                b"235 2.7.0 ok\r\n"
            } else if up.starts_with("MAIL")
                || up.starts_with("RCPT")
                || up.starts_with("RSET")
                || up.starts_with("NOOP")
            {
                b"250 ok\r\n"
            } else if up == "DATA" {
                in_data = true;
                b"354 go\r\n"
            } else if up == "QUIT" {
                wr.write_all(b"221 bye\r\n").await.unwrap();
                break;
            } else {
                b"502 no\r\n"
            };
            wr.write_all(reply).await.unwrap();
        }
    });
    (port, rec)
}

/// Boundaries renumbered by first appearance, the Date line blanked.
fn norm_data(s: &str) -> String {
    let re = regex::Regex::new(r"={15}\d{19}==").unwrap();
    let mut seen: Vec<String> = Vec::new();
    let s = re.replace_all(s, |c: &regex::Captures| {
        let b = c[0].to_string();
        let i = seen.iter().position(|x| *x == b).unwrap_or_else(|| {
            seen.push(b.clone());
            seen.len() - 1
        });
        format!("BOUNDARY{i}")
    });
    regex::Regex::new(r"(?m)^Date: .*\r$")
        .unwrap()
        .replace_all(&s, "Date: X\r")
        .to_string()
}

/// EHLO host and MAIL FROM extension parameters are the transport's own: aiosmtplib sends
/// `size=N BODY=8BITMIME` when offered, lettre sends neither for 7-bit content.
fn norm_cmd(c: &str) -> String {
    if c.starts_with("EHLO") {
        return "EHLO *".into();
    }
    c.to_string()
}

fn mail_from(c: &str) -> String {
    c.split(' ').take(2).collect::<Vec<_>>().join(" ")
}

#[tokio::test]
async fn an_smtp_session_carries_what_python_sends() {
    support::init();
    let mut cases = load("smtp");
    for case in cases.iter_mut() {
        let (port, rec) = sink().await;
        let c = &case["cfg"];
        let cfg = SmtpConfig {
            host: "127.0.0.1".into(),
            port: port as i64,
            user: c["user"].as_str().unwrap().into(),
            password: c["password"].as_str().unwrap().into(),
            from_name: c["from_name"].as_str().unwrap().into(),
            from_email: c["from_email"].as_str().unwrap().into(),
            tls: false,
        };
        let to: Vec<String> = match &case["to"] {
            Value::String(s) => vec![s.clone()],
            v => v
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect(),
        };
        let atts: Option<Vec<Attachment>> = case["attachments"].as_array().map(|a| {
            a.iter()
                .map(|x| Attachment {
                    filename: x["filename"].as_str().unwrap().into(),
                    content: base64::engine::general_purpose::STANDARD
                        .decode(x["content"].as_str().unwrap())
                        .unwrap(),
                    mime_type: None,
                })
                .collect()
        });
        let headers: Vec<(String, String)> = case["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|kv| {
                (
                    kv[0].as_str().unwrap().into(),
                    kv[1].as_str().unwrap().into(),
                )
            })
            .collect();
        let mut cap = Capture::default();
        let ok = email::_send_via_config(
            &cfg,
            &to,
            case["subject"].as_str().unwrap(),
            case["html"].as_str().unwrap(),
            None,
            atts.as_deref(),
            None,
            Some(&headers),
            Some(&mut cap),
        )
        .await
        .unwrap();
        assert!(ok);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (cmds, data) = rec.lock().unwrap().clone();
        if bless::blessing() {
            // the session Rust held is the reference now (the transcript and DATA as the sink saw them)
            // EHLO carries this machine's host name: record the normalised form, never the real name
            let recorded: Vec<String> = cmds.iter().map(|c| norm_cmd(c)).collect();
            case["transcript"] = serde_json::json!(recorded);
            case["data"] = serde_json::json!(data);
        }
        let want_cmds: Vec<String> = case["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        let norm = |v: &[String]| -> Vec<String> {
            v.iter()
                .map(|c| {
                    if c.starts_with("MAIL FROM:") {
                        mail_from(c)
                    } else {
                        norm_cmd(c)
                    }
                })
                .collect()
        };
        assert_eq!(norm(&cmds), norm(&want_cmds));
        assert_eq!(norm_data(&data), norm_data(case["data"].as_str().unwrap()));
        // The capture is the same message with "\n" line ends.
        let raw = String::from_utf8(cap.raw.unwrap()).unwrap();
        assert_eq!(norm_data(&raw.replace('\n', "\r\n")), norm_data(&data));
        assert_eq!(
            cap.message_id.as_deref(),
            Some(
                headers
                    .iter()
                    .find(|h| h.0 == "Message-ID")
                    .unwrap()
                    .1
                    .as_str()
            )
        );
    }
    bless::save("email/smtp.json", &Value::Array(cases));
}

#[tokio::test]
async fn config_is_read_from_settings_and_the_password_decrypted() {
    let pool = support::fresh_db("s4_email_cfg").await;
    let mut conn = pool.acquire().await.unwrap();
    // Nothing saved: Python's defaults.
    sqlx::query("DELETE FROM system_settings WHERE key LIKE 'email_smtp_%' OR key LIKE 'crm_smtp_%' OR key LIKE 'crm_imap_%'")
        .execute(&mut *conn)
        .await
        .unwrap();
    let cfg = email::load_system_smtp(&mut conn).await.unwrap();
    assert_eq!(
        cfg,
        SmtpConfig {
            host: "".into(),
            port: 587,
            user: "".into(),
            password: "".into(),
            from_name: "Granary".into(),
            from_email: "".into(),
            tls: true
        }
    );
    assert_eq!(
        email::load_crm_smtp(&mut conn).await.unwrap().from_name,
        "Your Company"
    );
    let imap = email::load_crm_imap(&mut conn).await.unwrap();
    assert_eq!((imap.port, imap.ssl), (993, true));

    let put = |k: &'static str, v: String| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO system_settings (id, key, value, category) VALUES ($1, $2, $3, 'email')")
                .bind(uuid::Uuid::new_v4())
                .bind(k)
                .bind(v)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    put("crm_smtp_host", "smtp.granary.test".into()).await;
    put("crm_smtp_port", " 465 ".into()).await;
    put("crm_smtp_password", email::encrypt("s3cret")).await;
    put("crm_smtp_tls", "False".into()).await;
    put("crm_smtp_from_name", "".into()).await;
    let cfg = email::load_crm_smtp(&mut conn).await.unwrap();
    assert_eq!(
        (cfg.host.as_str(), cfg.port, cfg.password.as_str(), cfg.tls),
        ("smtp.granary.test", 465, "s3cret", false)
    );
    assert_eq!(
        cfg.from_name, "",
        "a saved empty value is not replaced by the default"
    );

    put("crm_imap_port", "abc".into()).await;
    let err = email::load_crm_imap(&mut conn).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid literal for int() with base 10: 'abc'"
    );
}

/// The pattern of tests/test_document_delivery.py & co: `_send_via_config` and `load_system_smtp`
/// replaced; send_quotation_email goes through both with the template and the PDF attachment.
#[tokio::test]
async fn seams_replace_send_and_config_like_monkeypatch() {
    let pool = support::fresh_db("s4_email_seams").await;
    let mut conn = pool.acquire().await.unwrap();
    let sent: Arc<Mutex<Vec<SendCall>>> = Arc::default();
    let s2 = sent.clone();
    let seams = Seams {
        send_via_config: Some(Arc::new(move |call: SendCall| {
            s2.lock().unwrap().push(call.clone());
            Box::pin(async move {
                let cap = call.wants_capture.then(|| Capture {
                    raw: Some(b"raw message bytes".to_vec()),
                    message_id: None,
                });
                Ok((true, cap))
            })
        })),
        load_system_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    from_email: "erp@granary.test".into(),
                    from_name: "ERP".into(),
                    ..Default::default()
                })
            })
        })),
        load_crm_smtp: None,
    };
    let ok = email::with_seams(seams, async {
        email::send_quotation_email(
            "buyer@x.test",
            "Bob",
            "QUO-1",
            b"%PDF",
            &mut conn,
            "Acme Ltd",
            "hello@example.com",
            None,
        )
        .await
        .unwrap()
    })
    .await;
    assert!(ok);
    let calls = sent.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let c = &calls[0];
    assert_eq!(c.cfg.from_email, "erp@granary.test");
    assert_eq!(c.to, vec!["buyer@x.test".to_string()]);
    assert_eq!(c.subject, "Quotation QUO-1 - Acme Ltd");
    assert_eq!(
        c.html_body,
        granary_services::email_templates::quotation_email_template(
            "Bob",
            "QUO-1",
            "Acme Ltd",
            "hello@example.com",
            "https://example.com",
            "",
            ""
        )
    );
    let att = &c.attachments.as_ref().unwrap()[0];
    assert_eq!(
        (
            att.filename.as_str(),
            att.content.as_slice(),
            att.mime_type.as_deref()
        ),
        ("QUO-1.pdf", &b"%PDF"[..], Some("application/pdf"))
    );
    assert!(!c.has_db && c.headers.is_none() && !c.wants_capture);
}
