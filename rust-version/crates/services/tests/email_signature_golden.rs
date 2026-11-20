//! Golden replay: email_signature.rs and email_inline.rs against the results recorded from the real
//! Python functions (compat-tests/golden/gen_email_signature.py): placeholder extraction, field specs,
//! rendering, shell stripping, plain text, html.unescape, value resolution, inline_cid_images (real rows
//! in a fresh database, fake MinIO), import_external_images (wiremock serving what the Python run's
//! local server served, fake MinIO) and embed_inline_images (the raw messages Python built).

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use granary_services::email_inline;
use granary_services::email_signature::{
    self as es, AssetStore, InlinePart, SignatureUser, StoreError,
};
use serde_json::{json, Map, Value};

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/email_signature/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).expect("fixture JSON")
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[test]
fn placeholders_fields_render_and_plain_text() {
    let cases = load("pure");
    let mut bad = 0;
    for (i, c) in cases.iter().enumerate() {
        let tpl = c["template"].as_str().unwrap();
        let vals = obj(&c["values"]);
        let got = json!({
            "extract_placeholders": es::extract_placeholders(tpl),
            "field_spec": es::field_spec(tpl),
            "missing_fields": es::missing_fields(tpl, &vals),
            "render": es::render(tpl, &vals),
            "wrap": es::wrap(tpl),
            "to_plain_text": es::to_plain_text(tpl),
        });
        for k in [
            "extract_placeholders",
            "field_spec",
            "missing_fields",
            "render",
            "wrap",
            "to_plain_text",
        ] {
            if got[k] != c[k] {
                bad += 1;
                if bad < 6 {
                    eprintln!(
                        "case {i} {k}: template {tpl:?}\n want {}\n  got {}",
                        c[k], got[k]
                    );
                }
            }
        }
    }
    assert_eq!(bad, 0, "{bad} mismatches over {} cases", cases.len());
}

#[test]
fn document_shell_is_stripped_like_python() {
    let cases = load("shell");
    let mut bad = 0;
    for (i, c) in cases.iter().enumerate() {
        let html = c["html"].as_str().unwrap();
        for (k, got) in [
            ("strip_document_shell", es::strip_document_shell(html)),
            ("to_plain_text", es::to_plain_text(html)),
        ] {
            if c[k].as_str().unwrap() != got {
                bad += 1;
                if bad < 6 {
                    eprintln!(
                        "case {i} {k}: html {html:?}\n want {:?}\n  got {got:?}",
                        c[k]
                    );
                }
            }
        }
    }
    assert_eq!(bad, 0);
}

#[test]
fn html_unescape_is_pythons() {
    let cases = load("unescape");
    let mut bad = 0;
    for c in &cases {
        let got = es::html_unescape(c["in"].as_str().unwrap());
        if got != c["out"].as_str().unwrap() {
            bad += 1;
            eprintln!("in {:?}: want {:?} got {got:?}", c["in"], c["out"]);
        }
    }
    assert_eq!(bad, 0);
}

fn user_of(u: &Value) -> SignatureUser {
    let s = |k: &str| u.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    SignatureUser {
        name: s("name"),
        position: s("position"),
        department: s("department"),
        email: s("email"),
        phone: s("phone"),
        email_signature_enabled: None,
        email_signature_fields: u
            .get("email_signature_fields")
            .cloned()
            .filter(|v| !v.is_null()),
    }
}

#[test]
fn company_defaults_and_resolved_values() {
    for (i, c) in load("values").iter().enumerate() {
        let company = obj(&c["company"]);
        let user = user_of(&c["user"]);
        assert_eq!(
            Value::Object(es::company_values(&company)),
            c["company_values"],
            "case {i} company_values"
        );
        assert_eq!(
            Value::Object(es::defaults_for_user(&user, Some(&company))),
            c["defaults_for_user"],
            "case {i}"
        );
        assert_eq!(
            Value::Object(es::resolved_values(&user, &company)),
            c["resolved_values"],
            "case {i}"
        );
    }
}

// ─── Fake MinIO ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeStore {
    objects: HashMap<String, Option<Vec<u8>>>,
    fail_puts: bool,
    buckets: Mutex<Vec<String>>,
    puts: Mutex<Vec<Value>>,
}

#[async_trait]
impl AssetStore for FakeStore {
    async fn bucket_exists(&self, bucket: &str) -> Result<bool, StoreError> {
        Ok(self.buckets.lock().unwrap().iter().any(|b| b == bucket))
    }
    async fn make_bucket(&self, bucket: &str) -> Result<(), StoreError> {
        self.buckets.lock().unwrap().push(bucket.to_string());
        Ok(())
    }
    async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), StoreError> {
        if self.fail_puts {
            return Err(StoreError {
                kind: "S3Error".into(),
                message: "denied".into(),
            });
        }
        let enc = base64::engine::general_purpose::STANDARD.encode(data);
        self.puts.lock().unwrap().push(json!({
            "bucket": bucket, "key": norm_key(key), "data": enc.chars().take(40).collect::<String>(),
            "length": data.len(), "content_type": content_type,
        }));
        Ok(())
    }
    async fn get_object(&self, _bucket: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        match self.objects.get(key) {
            Some(Some(d)) => Ok(d.clone()),
            _ => Err(StoreError {
                kind: "S3Error".into(),
                message: "NoSuchKey".into(),
            }),
        }
    }
}

fn norm_key(key: &str) -> String {
    let re = regex::Regex::new(r"email-signature/[0-9a-f-]{36}/").unwrap();
    re.replace(key, "email-signature/{UUID}/").to_string()
}

#[tokio::test]
async fn inline_cid_images_swaps_our_images_for_cid_parts() {
    let pool = support::fresh_db("s4_sig_inline").await;
    let assets: Vec<Value> = {
        let path = format!(
            "{}/../../compat-tests/golden/email_signature/assets.json",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let mut store = FakeStore::default();
    for a in &assets {
        sqlx::query("INSERT INTO email_signature_assets (id, filename, mime_type, object_key, file_size) VALUES ($1,$2,$3,$4,1)")
            .bind(uuid::Uuid::parse_str(a["id"].as_str().unwrap()).unwrap())
            .bind(a["filename"].as_str().unwrap())
            .bind(a["mime_type"].as_str().unwrap())
            .bind(a["object_key"].as_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        store.objects.insert(
            a["object_key"].as_str().unwrap().to_string(),
            a["data"].as_str().map(b64),
        );
    }
    let store: Arc<dyn AssetStore> = Arc::new(store);
    let cases = load("inline_cid");
    let mut conn = pool.acquire().await.unwrap();
    es::with_asset_store(store, async {
        for (i, c) in cases.iter().enumerate() {
            let (out, parts) = es::inline_cid_images(&mut conn, c["html"].as_str().unwrap()).await;
            assert_eq!(out, c["out"].as_str().unwrap(), "case {i}: {:?}", c["html"]);
            let want: Vec<InlinePart> = c["parts"]
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
            assert_eq!(parts, want, "case {i}");
        }
    })
    .await;
}

#[tokio::test]
async fn import_external_images_matches_python() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let pool = support::fresh_db("s4_sig_import").await;
    granary_services::safe_fetch::allow_loopback_for_tests();
    let server = MockServer::start().await;
    let big = vec![0u8; 5 * 1024 * 1024 + 1];
    let routes: Vec<(&str, u16, Option<&str>, Vec<u8>)> = vec![
        (
            "/logo.png",
            200,
            Some("image/png"),
            b"\x89PNG logo".to_vec(),
        ),
        (
            "/img/photo.jpg",
            200,
            Some("image/jpeg; charset=binary"),
            b"\xff\xd8 photo".to_vec(),
        ),
        (
            "/page.html",
            200,
            Some("text/html; charset=utf-8"),
            b"<html></html>".to_vec(),
        ),
        ("/missing.png", 404, Some("text/plain"), b"nope".to_vec()),
        ("/big.png", 200, Some("image/png"), big),
        ("/noctype", 200, None, b"data".to_vec()),
        ("/dir/", 200, Some("image/svg+xml"), b"<svg/>".to_vec()),
        ("/err500.png", 500, Some("text/plain"), b"x".to_vec()),
    ];
    for (p, status, ctype, body) in routes {
        let mut r = ResponseTemplate::new(status).set_body_bytes(body);
        if let Some(ct) = ctype {
            r = r.insert_header("content-type", ct);
        }
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(r)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/logo.png"))
        .mount(&server)
        .await;
    let base = server.uri();

    let mut conn = pool.acquire().await.unwrap();
    for (i, c) in load("import").iter().enumerate() {
        sqlx::query("DELETE FROM email_signature_assets")
            .execute(&mut *conn)
            .await
            .unwrap();
        let store = Arc::new(FakeStore {
            fail_puts: c["store_fails"].as_bool().unwrap(),
            ..Default::default()
        });
        let html = c["html"].as_str().unwrap().replace("{B}", &base);
        let (out, n, problems) =
            es::with_asset_store(store.clone(), es::import_external_images(&mut conn, &html))
                .await
                .unwrap();
        let rows: Vec<(uuid::Uuid, String, String, String, Option<i32>, Option<String>)> = sqlx::query_as(
            "SELECT id, filename, mime_type, object_key, file_size, source_url FROM email_signature_assets ORDER BY created_at, ctid",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        let mut out = out.replace(&base, "{B}");
        for (k, r) in rows.iter().enumerate() {
            out = out.replace(&r.0.to_string(), &format!("{{ASSET{k}}}"));
        }
        assert_eq!(out, c["out"].as_str().unwrap(), "case {i} html");
        assert_eq!(n, c["imported"].as_i64().unwrap(), "case {i} imported");
        let problems: Vec<String> = problems.iter().map(|p| p.replace(&base, "{B}")).collect();
        assert_eq!(json!(problems), c["problems"], "case {i} problems");
        let got_rows: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({"filename": r.1, "mime_type": r.2, "object_key": norm_key(&r.3), "file_size": r.4,
                       "source_url": r.5.as_deref().unwrap_or("").replace(&base, "{B}")})
            })
            .collect();
        assert_eq!(json!(got_rows), c["rows"], "case {i} rows");
        assert_eq!(
            json!(*store.puts.lock().unwrap()),
            c["puts"],
            "case {i} puts"
        );
    }
}

/// Security triage #21: a signature image on an internal address (cloud metadata, a private network) is
/// refused before any request is sent; nothing is stored and the template keeps the original src.
#[tokio::test]
async fn import_external_images_refuses_internal_addresses() {
    let pool = support::fresh_db("s4_sig_ssrf").await;
    let mut conn = pool.acquire().await.unwrap();
    let store = Arc::new(FakeStore::default());
    let html = "<img src=\"http://169.254.169.254/latest/meta-data/x.png\"><img src=\"http://10.1.2.3/logo.png\">\
                <img src=\"http://[::ffff:192.168.0.1]/a.png\">";
    let (out, n, problems) =
        es::with_asset_store(store.clone(), es::import_external_images(&mut conn, html))
            .await
            .unwrap();
    assert_eq!(n, 0);
    assert_eq!(out, html);
    assert_eq!(problems.len(), 3, "{problems:?}");
    for p in &problems {
        assert!(
            p.contains("refused (") && p.contains("private or internal"),
            "{p}"
        );
    }
    assert!(store.puts.lock().unwrap().is_empty());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM email_signature_assets")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn embed_inline_images_matches_python() {
    for (i, c) in load("embed").iter().enumerate() {
        let raw = b64(c["raw"].as_str().unwrap());
        let parsed = mailparse::parse_mail(&raw).unwrap();
        let got = email_inline::embed_inline_images(&parsed, c["html"].as_str());
        assert_eq!(json!(got), c["out"], "case {i}: {:?}", c["html"]);
    }
}

/// tests/test_mail_sync.py::test_inline_cid_images_are_embedded_and_oversized_ones_left_alone (the
/// embedding half; `crm_inbox._extract_body_and_attachments` wraps exactly this call).
#[test]
fn test_inline_cid_images_are_embedded_and_oversized_ones_left_alone() {
    let enc = |d: &[u8]| {
        let s = base64::engine::general_purpose::STANDARD.encode(d);
        s.as_bytes()
            .chunks(76)
            .map(|c| std::str::from_utf8(c).unwrap().to_string() + "\n")
            .collect::<String>()
    };
    let html =
        r#"<p>Hi</p><img src="cid:_Foxmail.1@a2be"><img src="cid:big@x"><img src="cid:missing@x">"#;
    let big = vec![b'x'; email_inline::MAX_EACH + 1];
    let raw = format!(
        "Subject: Logo\nMIME-Version: 1.0\nContent-Type: multipart/alternative; boundary=\"A\"\n\n\
--A\nContent-Type: text/plain; charset=\"utf-8\"\n\nplain\n\
--A\nContent-Type: multipart/related; boundary=\"B\"\n\n\
--B\nContent-Type: text/html; charset=\"utf-8\"\n\n{html}\n\
--B\nContent-Type: image/png\nContent-Transfer-Encoding: base64\nContent-ID: <_Foxmail.1@a2be>\n\n{}\
--B\nContent-Type: image/png\nContent-Transfer-Encoding: base64\nContent-ID: <big@x>\n\n{}\
--B--\n--A--\n",
        enc(b"\x89PNG small"),
        enc(&big)
    );
    let parsed = mailparse::parse_mail(raw.as_bytes()).unwrap();
    let body_html = email_inline::embed_inline_images(&parsed, Some(html)).unwrap();
    assert!(
        body_html.contains(r#"src="data:image/png;base64,"#) && !body_html.contains("cid:_Foxmail")
    );
    assert!(
        body_html.contains(r#"src="cid:big@x""#),
        "too large to embed: left as a reference"
    );
    assert!(body_html.contains(r#"src="cid:missing@x""#));
}

/// The 5 MB budget per message: the fourth 1.4 MB image no longer fits (Python: same arithmetic).
#[test]
fn the_message_budget_stops_embedding() {
    let enc = |d: &[u8]| {
        let s = base64::engine::general_purpose::STANDARD.encode(d);
        s.as_bytes()
            .chunks(76)
            .map(|c| std::str::from_utf8(c).unwrap().to_string() + "\n")
            .collect::<String>()
    };
    let img = vec![b'y'; 1_400_000];
    let mut raw = String::from("MIME-Version: 1.0\nContent-Type: multipart/related; boundary=\"B\"\n\n--B\nContent-Type: text/html\n\nx\n");
    for k in 0..4 {
        raw.push_str(&format!("--B\nContent-Type: image/gif\nContent-Transfer-Encoding: base64\nContent-ID: <i{k}>\n\n{}", enc(&img)));
    }
    raw.push_str("--B--\n");
    let parsed = mailparse::parse_mail(raw.as_bytes()).unwrap();
    let html = r#"<img src="cid:i0"><img src='cid:i1'><img src="cid:i2"><img src="cid:i3">"#;
    let out = email_inline::embed_inline_images(&parsed, Some(html)).unwrap();
    assert_eq!(out.matches("data:image/gif;base64,").count(), 3);
    assert!(out.ends_with(r#"<img src="cid:i3">"#));
}
