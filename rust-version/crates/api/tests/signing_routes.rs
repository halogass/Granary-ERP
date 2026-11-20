//! routers/signing.py and routers/signing_flow.py through the real app. Ports of the backend tests that
//! seed rows directly or need fake sidecars:
//! - test_bughunt_fixes.py: the four e-signature tests;
//! - test_audit_medium_w3.py: test_only_the_owner_manages_a_sign_document,
//!   test_signers_and_fields_change_only_on_drafts, test_the_certificate_includes_the_final_signers_events;
//! - test_audit_high_b2.py: test_certificate_html_escapes_every_value,
//!   test_signature_data_must_be_a_small_png_data_url;
//! - test_audit_critical.py: test_signer_tokens_are_only_shown_to_that_signer;
//! - test_sign_from_document.py: the route-level tests (unapproved, extension cap) and the two
//!   public-signer-page tests the services crate left to the routers wave.
//!
//! One fake server (wiremock, own thread) stands in for the PDF service (`/render` for the verification
//! render, `/pdf` for the certificate, recording the HTML) and MinIO (path-style S3). The PAdES seal uses
//! a self-signed certificate generated in a scratch path; the sealed PDF is validated with
//! pdf_sign::validate_pdf (a new seal cannot be byte-identical to pyHanko's).

mod support;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{u, TestApp, UserSpec};
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

const TAG: &str = "signing";

static STORE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
static CERT_HTML: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Fake;

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let path = req.url.path().to_string();
        let method = req.method.as_str().to_string();
        if path == "/render" {
            return ResponseTemplate::new(200).set_body_bytes(anchors_pdf());
        }
        if path == "/pdf" {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            CERT_HTML
                .lock()
                .unwrap()
                .push(body["html"].as_str().unwrap_or("").to_string());
            return ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.4 certificate".to_vec());
        }
        let mut store = STORE.lock().unwrap();
        let store = store.get_or_insert_with(HashMap::new);
        let rest = path.trim_start_matches('/');
        match (method.as_str(), rest.split_once('/')) {
            ("HEAD", None) | ("PUT", None) => ResponseTemplate::new(200),
            ("PUT", Some((_, key))) => {
                store.insert(key.to_string(), req.body.clone());
                ResponseTemplate::new(200)
            }
            ("GET", Some((_, key))) => match store.get(key) {
                Some(b) => ResponseTemplate::new(200).set_body_bytes(b.clone()),
                None => ResponseTemplate::new(404),
            },
            _ => ResponseTemplate::new(404),
        }
    }
}

fn anchors_pdf() -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../compat-tests/golden/verification_delivery/anchors.pdf"),
    )
    .unwrap()
}

/// The app, with the fake sidecars in place (env set before the first `support::app`).
fn app() -> &'static TestApp {
    static URI: OnceLock<String> = OnceLock::new();
    URI.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let server = wiremock::MockServer::start().await;
                wiremock::Mock::given(wiremock::matchers::any())
                    .respond_with(Fake)
                    .mount(&server)
                    .await;
                tx.send(server.uri()).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let uri = rx.recv().unwrap();
        let p12 =
            std::env::temp_dir().join(format!("granary-api-signing-{}.p12", std::process::id()));
        std::env::set_var("PDF_SERVICE_URL", &uri);
        std::env::set_var("MINIO_ENDPOINT", uri.trim_start_matches("http://"));
        std::env::set_var("MINIO_USE_SSL", "false");
        std::env::set_var("APP_URL", "http://test.local");
        std::env::set_var("SIGN_CERT_P12_PATH", p12);
        uri
    });
    support::app(TAG)
}

fn stored(key: &str) -> Option<Vec<u8>> {
    STORE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(key)
        .cloned()
}

fn put(key: &str, data: Vec<u8>) {
    STORE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(key.to_string(), data);
}

/// A sign document with signers `(sign_order, status)`; returns (doc id, signer ids, tokens).
async fn sign_doc(
    db: &mut PgConnection,
    signers: &[(i32, &str)],
    expires_at: Option<chrono::DateTime<Utc>>,
    owner: Option<Uuid>,
    status: &str,
) -> (Uuid, Vec<Uuid>, Vec<String>) {
    let doc = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sign_documents (id, title, status, expires_at, created_by_id, signature_level, jurisdictions, \
         is_deleted) VALUES ($1, $2, $3, $4, $5, 'ses', 'US,EU,ID', false)",
    )
    .bind(doc)
    .bind(u("Pytest bughunt"))
    .bind(status)
    .bind(expires_at)
    .bind(owner)
    .execute(&mut *db)
    .await
    .unwrap();
    let (mut ids, mut tokens) = (vec![], vec![]);
    for (i, (order, st)) in signers.iter().enumerate() {
        let id = Uuid::new_v4();
        let token = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO sign_signers (id, document_id, name, email, sign_order, role, token, status, signer_type) \
             VALUES ($1, $2, $3, $4, $5, 'signer', $6, $7, 'external')",
        )
        .bind(id)
        .bind(doc)
        .bind(format!("Signer {i}"))
        .bind(format!("s{i}@bughunt.test"))
        .bind(order)
        .bind(token)
        .bind(st)
        .execute(&mut *db)
        .await
        .unwrap();
        ids.push(id);
        tokens.push(token.to_string());
    }
    (doc, ids, tokens)
}

fn sign_body(field_values: Value) -> Value {
    json!({"signature_data": null, "consent": true, "field_values": field_values})
}

async fn status_of(db: &mut PgConnection, table: &str, id: Uuid) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT status FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

// ─── test_bughunt_fixes.py ────────────────────────────────────────────────────

#[tokio::test]
async fn test_an_expired_link_cannot_sign_or_decline() {
    let app = app();
    let mut db = app.db().await;
    let (_, _, tokens) = sign_doc(
        &mut db,
        &[(1, "sent")],
        Some(Utc::now() - Duration::hours(1)),
        None,
        "pending",
    )
    .await;
    let r = app
        .post(
            "",
            &format!("/api/v1/sign/public/{}/sign", tokens[0]),
            sign_body(json!([])),
        )
        .await;
    assert_eq!(r.status, 410, "{}", r.text());
    let r = app
        .request(
            "POST",
            &format!("/api/v1/sign/public/{}/decline", tokens[0]),
            "",
            None,
        )
        .await;
    assert_eq!(r.status, 410);
    assert_eq!(r.json(), json!({"detail": "This signing link has expired"}));
}

#[tokio::test]
async fn test_the_next_step_waits_for_the_whole_current_step() {
    let app = app();
    let mut db = app.db().await;
    let (_, ids, tokens) = sign_doc(
        &mut db,
        &[(1, "sent"), (1, "sent"), (2, "pending")],
        None,
        None,
        "pending",
    )
    .await;
    let r = app
        .post(
            "",
            &format!("/api/v1/sign/public/{}/sign", tokens[0]),
            sign_body(json!([])),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["document_completed"], false);
    let early = app
        .post(
            "",
            &format!("/api/v1/sign/public/{}/sign", tokens[2]),
            sign_body(json!([])),
        )
        .await;
    assert_eq!(early.status, 400);
    assert_eq!(
        early.json(),
        json!({"detail": "This document is not waiting for your signature yet"})
    );
    assert_eq!(
        status_of(&mut db, "sign_signers", ids[2]).await.as_deref(),
        Some("pending")
    );
}

#[tokio::test]
async fn test_a_signer_cannot_fill_another_signers_field() {
    let app = app();
    let mut db = app.db().await;
    let (doc, ids, tokens) =
        sign_doc(&mut db, &[(1, "sent"), (1, "sent")], None, None, "pending").await;
    let (theirs, mine, open) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    for (id, signer) in [(theirs, Some(ids[1])), (mine, Some(ids[0])), (open, None)] {
        sqlx::query(
            "INSERT INTO sign_fields (id, document_id, signer_id, field_type, page_number, is_required) \
             VALUES ($1, $2, $3, 'text', 1, true)",
        )
        .bind(id)
        .bind(doc)
        .bind(signer)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = app
        .post(
            "",
            &format!("/api/v1/sign/public/{}/sign", tokens[0]),
            sign_body(json!([
                {"field_id": theirs.to_string(), "value": "forged"},
                {"field_id": mine.to_string(), "value": "mine"},
                {"field_id": open.to_string(), "value": true},
                {"field_id": open.to_string(), "value": "second"},
                {"field_id": "", "value": "ignored"},
            ])),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let value = |id: Uuid| async move {
        let mut db = app.db().await;
        sqlx::query_scalar::<_, Option<String>>("SELECT value FROM sign_fields WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap()
    };
    assert_eq!(value(theirs).await, None);
    assert_eq!(value(mine).await.as_deref(), Some("mine"));
    // str(True), and the second value does not overwrite a field the first one filled
    assert_eq!(value(open).await.as_deref(), Some("True"));
}

#[tokio::test]
async fn test_a_finished_document_cannot_be_declined() {
    let app = app();
    let mut db = app.db().await;
    let (doc, _, tokens) = sign_doc(&mut db, &[(1, "sent")], None, None, "voided").await;
    let r = app
        .request(
            "POST",
            &format!("/api/v1/sign/public/{}/decline", tokens[0]),
            "",
            None,
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "Document is voided: cannot decline"})
    );
    assert_eq!(
        status_of(&mut db, "sign_documents", doc).await.as_deref(),
        Some("voided")
    );
}

#[tokio::test]
async fn decline_records_the_reason_and_the_trail() {
    let app = app();
    let mut db = app.db().await;
    let (doc, ids, tokens) = sign_doc(&mut db, &[(1, "sent")], None, None, "pending").await;
    let r = app
        .send(
            app.client()
                .post(app.url(&format!(
                    "/api/v1/sign/public/{}/decline?reason=Too%20late",
                    tokens[0]
                )))
                .header("user-agent", "pytest-agent")
                .header("x-real-ip", "10.1.2.3"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"status": "declined"}));
    let row: (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT status, declined_reason, ip_address, user_agent FROM sign_signers WHERE id = $1",
    )
    .bind(ids[0])
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            Some("declined".into()),
            Some("Too late".into()),
            Some("10.1.2.3".into()),
            Some("pytest-agent".into())
        )
    );
    assert_eq!(
        status_of(&mut db, "sign_documents", doc).await.as_deref(),
        Some("declined")
    );
    let desc: String = sqlx::query_scalar(
        "SELECT description FROM sign_audit_logs WHERE document_id = $1 AND action = 'document_declined'",
    )
    .bind(doc)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(desc, "Signer 0 declined: Too late");
    let again = app
        .request(
            "POST",
            &format!("/api/v1/sign/public/{}/decline", tokens[0]),
            "",
            None,
        )
        .await;
    assert_eq!(again.json(), json!({"detail": "Already declined"}));
}

// ─── test_audit_medium_w3.py ──────────────────────────────────────────────────

#[tokio::test]
async fn test_only_the_owner_manages_a_sign_document() {
    let app = app();
    let owner = app.user_with(&["sign:manage"]).await;
    let other = app.user_with(&["sign:manage"]).await;
    let mut db = app.db().await;
    let (doc, ids, _) = sign_doc(&mut db, &[(1, "pending")], None, Some(owner.id), "draft").await;
    let d = format!("/api/v1/sign/documents/{doc}");
    let t = &other.token;
    assert_eq!(app.delete(t, &d).await.status, 403);
    assert_eq!(
        app.post(
            t,
            &format!("{d}/signers"),
            json!({"name": "X", "email": "x@example.test"})
        )
        .await
        .status,
        403
    );
    assert_eq!(
        app.delete(t, &format!("{d}/signers/{}", ids[0]))
            .await
            .status,
        403
    );
    assert_eq!(
        app.post(
            t,
            &format!("{d}/fields"),
            json!({"field_type": "signature"})
        )
        .await
        .status,
        403
    );
    assert_eq!(
        app.post(t, &format!("{d}/fields/batch"), json!({"fields": []}))
            .await
            .status,
        403
    );
    assert_eq!(
        app.request("POST", &format!("{d}/send"), t, None)
            .await
            .status,
        403
    );
    assert_eq!(
        app.request("POST", &format!("{d}/remind?signer_id={}", ids[0]), t, None)
            .await
            .status,
        403
    );
}

#[tokio::test]
async fn test_signers_and_fields_change_only_on_drafts() {
    let app = app();
    let owner = app.user_with(&["sign:manage"]).await;
    let mut db = app.db().await;
    let (doc, ids, _) = sign_doc(&mut db, &[(1, "sent")], None, Some(owner.id), "pending").await;
    let d = format!("/api/v1/sign/documents/{doc}");
    let r = app
        .delete(&owner.token, &format!("{d}/signers/{}", ids[0]))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "Can only remove signers from draft documents"})
    );
    let r = app
        .post(
            &owner.token,
            &format!("{d}/fields/batch"),
            json!({"fields": []}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "Can only edit the fields of draft documents"})
    );
}

#[tokio::test]
async fn test_the_certificate_includes_the_final_signers_events() {
    let app = app();
    let _g = app.serial().await;
    CERT_HTML.lock().unwrap().clear();
    let owner = app.user_with(&["sign:manage"]).await;
    let mut db = app.db().await;
    let (doc, _, tokens) = sign_doc(&mut db, &[(1, "sent")], None, Some(owner.id), "pending").await;
    let r = app
        .post(
            "",
            &format!("/api/v1/sign/public/{}/sign", tokens[0]),
            json!({"consent": true}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["document_completed"], true);
    let html = CERT_HTML.lock().unwrap().clone();
    let html = html
        .iter()
        .find(|h| h.contains(&doc.to_string()))
        .expect("certificate rendered");
    for event in [
        "Consent Given",
        "Document Signed",
        "Document Completed",
        "Intent Affirmed",
    ] {
        assert!(html.contains(event), "{event} missing from the certificate");
    }
    // no original PDF: no signed PDF, but the certificate is stored and recorded
    let row: (Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT signed_file_url, certificate_url, certificate_generated_at IS NOT NULL FROM sign_documents WHERE id = $1",
    )
    .bind(doc)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        row,
        (None, Some(format!("signing/certificates/{doc}.pdf")), true)
    );
    assert!(stored(&format!("certificates/{doc}.pdf")).is_some());
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM sign_audit_logs WHERE document_id = $1 ORDER BY created_at",
    )
    .bind(doc)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert!(actions.contains(&"certificate_generated".to_string()));
    // the signer can now download it
    let cert = app
        .get(
            "",
            &format!("/api/v1/sign/public/{}/certificate", tokens[0]),
        )
        .await;
    assert_eq!(cert.status, 200);
    assert_eq!(cert.body, b"%PDF-1.4 certificate");
    assert!(cert
        .header("content-disposition")
        .unwrap()
        .starts_with("attachment; filename=\"Pytest bughunt-"));
}

// ─── test_audit_high_b2.py ────────────────────────────────────────────────────

#[test]
fn test_certificate_html_escapes_every_value() {
    use granary_db::rows::signing::{SignAuditLog, SignDocument, SignSigner};
    let evil = "<script>alert(1)</script>";
    let now = Utc::now();
    let doc: SignDocument = serde_json::from_value(json!({
        "title": format!("Contract {evil}"), "status": "completed", "source_type": null, "source_id": null,
        "original_file_url": null, "signed_file_url": null, "certificate_url": null,
        "certificate_generated_at": null, "sealed_at": null, "seal_fingerprint": null, "document_hash": evil,
        "signature_level": null, "seal_level": null, "jurisdictions": "US,EU,ID", "message": null,
        "expires_at": null, "completed_at": null, "created_by_id": null, "id": Uuid::new_v4(),
        "created_at": now, "updated_at": now, "is_deleted": false, "deleted_at": null,
    }))
    .unwrap();
    let signer = |name: &str,
                  email: &str,
                  sig: &str,
                  ip: Option<&str>,
                  consent: Option<&str>|
     -> SignSigner {
        serde_json::from_value(json!({
            "document_id": doc.id, "name": name, "email": email, "sign_order": 1, "role": "signer",
            "token": Uuid::new_v4(), "status": "signed", "sent_at": null, "viewed_at": null, "signed_at": null,
            "declined_reason": null, "ip_address": ip, "user_agent": null, "user_id": null,
            "signer_type": "external", "partner_account_id": null, "signature_data": sig, "initials_data": null,
            "sign_reason": null, "consent_version": consent,
            "consent_agreed_at": consent.map(|_| now), "consent_text_sha256": null,
            "id": Uuid::new_v4(), "created_at": now, "updated_at": now,
        }))
        .unwrap()
    };
    let good_png = "data:image/png;base64,iVBORyBmYWtl";
    let signers = vec![
        signer(
            evil,
            &format!("x{evil}@y.z"),
            "javascript:alert(1)\" onerror=\"alert(1)",
            Some(evil),
            Some(evil),
        ),
        signer("Ok", "ok@y.z", good_png, None, None),
    ];
    let logs: Vec<SignAuditLog> = vec![serde_json::from_value(json!({
        "document_id": doc.id, "signer_id": null, "action": "document_signed", "description": evil,
        "ip_address": evil, "user_agent": null, "id": Uuid::new_v4(), "created_at": now, "updated_at": now,
    }))
    .unwrap()];
    let html = granary_api::routers::signing::build_certificate_html(
        &doc, &signers, &logs, now, "", "Acme Ltd",
    );
    assert!(!html.contains("<script>") && html.contains("&lt;script&gt;"));
    assert!(!html.contains("javascript:") && !html.contains("onerror=\""));
    assert!(html.contains(&format!("<img src=\"{good_png}\"")));
    assert!(html.contains("<br>&bull; United States: ESIGN Act"));
}

#[tokio::test]
async fn test_signature_data_must_be_a_small_png_data_url() {
    let app = app();
    let _g = app.serial().await;
    app.set_setting("sign.signature_max_bytes", "1000").await;
    let mut db = app.db().await;
    let (_, ids, tokens) = sign_doc(&mut db, &[(1, "sent")], None, None, "pending").await;
    let big = format!(
        "data:image/png;base64,{}",
        "eHh4".repeat(334) // 1002 bytes
    );
    let cases = [
        (
            "javascript:alert(1)",
            "signature_data must be a PNG image (data:image/png;base64,\u{2026})",
        ),
        (
            "data:image/svg+xml;base64,PHN2Zz4=",
            "signature_data must be a PNG image (data:image/png;base64,\u{2026})",
        ),
        (
            "data:image/png;base64,@@not-base64@@",
            "signature_data is not valid base64",
        ),
        (
            "data:image/png;base64,eHh",
            "signature_data is not valid base64",
        ),
        (
            "data:image/png;base64,",
            "signature_data must be a non-empty image of at most 1000 bytes",
        ),
        (
            big.as_str(),
            "signature_data must be a non-empty image of at most 1000 bytes",
        ),
    ];
    for (value, detail) in cases {
        let r = app
            .post(
                "",
                &format!("/api/v1/sign/public/{}/sign", tokens[0]),
                json!({"signature_data": value, "consent": true}),
            )
            .await;
        assert_eq!(r.status, 422, "{value}: {}", r.text());
        assert_eq!(r.json(), json!({"detail": detail}), "{value}");
    }
    let r = app
        .post(
            "",
            &format!("/api/v1/sign/public/{}/sign", tokens[0]),
            json!({"initials_data": "nope", "consent": true}),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"detail": "initials_data must be a PNG image (data:image/png;base64,\u{2026})"})
    );
    let row: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT status, signature_data FROM sign_signers WHERE id = $1")
            .bind(ids[0])
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(row, (Some("sent".into()), None));
    let r = app
        .post(
            "",
            &format!("/api/v1/sign/public/{}/sign", tokens[0]),
            json!({"consent": false}),
        )
        .await;
    assert_eq!(r.status, 400);
}

// ─── test_audit_critical.py ───────────────────────────────────────────────────

#[tokio::test]
async fn test_signer_tokens_are_only_shown_to_that_signer() {
    let app = app();
    let owner = app
        .make_user(UserSpec::new().perms(&["sign:manage"]).email(&format!(
            "pytest.owner.{}@example.com",
            &Uuid::new_v4().simple().to_string()[..6]
        )))
        .await;
    let snoop = app.user_with(&["sign:view"]).await;
    let mut db = app.db().await;
    let doc = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sign_documents (id, title, status, created_by_id, is_deleted) VALUES ($1, 'Pytest C5', 'pending', $2, false)",
    )
    .bind(doc)
    .bind(owner.id)
    .execute(&mut db)
    .await
    .unwrap();
    for (name, email, order) in [
        ("Client", "client@example.com".to_string(), 1),
        ("Owner", owner.email.to_uppercase(), 2),
    ] {
        sqlx::query(
            "INSERT INTO sign_signers (id, document_id, name, email, sign_order, token, status, role) \
             VALUES ($1, $2, $3, $4, $5, $6, 'pending', 'signer')",
        )
        .bind(Uuid::new_v4())
        .bind(doc)
        .bind(name)
        .bind(email)
        .bind(order)
        .bind(Uuid::new_v4())
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = app
        .get(&snoop.token, &format!("/api/v1/sign/documents/{doc}"))
        .await;
    assert_eq!(r.status, 404);
    let r = app
        .get(&owner.token, &format!("/api/v1/sign/documents/{doc}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let tokens: HashMap<String, Value> = body["signers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap().to_string(), s["token"].clone()))
        .collect();
    assert_eq!(tokens["Client"], Value::Null);
    assert!(tokens["Owner"].is_string());
}

// ─── test_sign_from_document.py ───────────────────────────────────────────────

/// The seeded superadmin (the Owner the approval routing picks).
async fn owner_token(app: &TestApp) -> String {
    let mut db = app.db().await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&granary_core::config::settings().SUPERADMIN_EMAIL)
        .fetch_one(&mut db)
        .await
        .unwrap();
    app.token_for(id)
}

/// `_contract()`: a draft service contract for an organization with a legal email.
async fn contract(db: &mut PgConnection) -> Uuid {
    let org = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, email, is_deleted) VALUES ($1, $2, $3, false)",
    )
    .bind(org)
    .bind(u("Pytest Customer"))
    .bind(format!("{}@customer.test", u("legal")))
    .execute(&mut *db)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contracts (id, number, customer_org_id, status, contract_type, value, currency, start_date, end_date, \
         auto_renew, is_deleted) VALUES ($1, $2, $3, 'draft', 'service', 250000000, 'JPY', '2026-10-01', '2027-09-30', false, false)",
    )
    .bind(id)
    .bind(u("PYT-CT"))
    .bind(org)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// `_approve(token, entity_type, entity_id)` through /approvals.
async fn approve(app: &TestApp, token: &str, entity_type: &str, id: Uuid) {
    let r = app
        .post(
            token,
            "/api/v1/approvals/submit",
            json!({"entity_type": entity_type, "entity_id": id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let body = r.json();
    if body["status"] == "pending" {
        let r = app
            .post(
                token,
                &format!("/api/v1/approvals/{}/approve", body["id"].as_str().unwrap()),
                json!({}),
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
        assert_eq!(r.json()["status"], "approved");
    }
}

async fn start(app: &TestApp, token: &str, entity_type: &str, id: Uuid) -> support::Resp {
    app.post(
        token,
        "/api/v1/sign/from-document",
        json!({"entity_type": entity_type, "entity_id": id.to_string()}),
    )
    .await
}

#[tokio::test]
async fn test_unapproved_document_cannot_be_sent_for_signature() {
    let app = app();
    let token = owner_token(app).await;
    let mut db = app.db().await;
    let id = contract(&mut db).await;
    let r = start(app, &token, "sales_contract", id).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.json()["detail"]["code"], "approval_required");
}

#[tokio::test]
async fn test_contract_deadline_can_be_extended_up_to_the_policy_cap() {
    let app = app();
    let token = owner_token(app).await;
    let mut db = app.db().await;
    let id = contract(&mut db).await;
    approve(app, &token, "sales_contract", id).await;
    let r = start(app, &token, "sales_contract", id).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let doc = r.json()["sign_document"].clone();
    let signers = doc["signers"].as_array().unwrap();
    assert_eq!(signers.len(), 1);
    assert!(signers[0]["email"]
        .as_str()
        .unwrap()
        .ends_with("@customer.test"));
    assert_eq!(doc["can_extend"], true);
    let doc_id = doc["id"].as_str().unwrap();
    let ok = app
        .post(
            &token,
            &format!("/api/v1/sign/documents/{doc_id}/extend"),
            json!({"days": 5}),
        )
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert!(
        ok.json()["sign_document"]["expires_at"].as_str().unwrap()
            > doc["expires_at"].as_str().unwrap()
    );
    let too_far = app
        .post(
            &token,
            &format!("/api/v1/sign/documents/{doc_id}/extend"),
            json!({"days": 10}),
        )
        .await;
    assert_eq!(too_far.status, 400);
    assert_eq!(
        status_of(&mut db, "contracts", id).await.as_deref(),
        Some("sent_for_signing")
    );
    let got = app
        .get(
            &token,
            &format!("/api/v1/sign/from-document/sales_contract/{id}"),
        )
        .await;
    assert_eq!(got.json()["sign_document"]["id"], doc["id"]);
}

#[tokio::test]
async fn test_signer_signs_through_the_public_page_and_the_stamp_lands_in_the_field() {
    let app = app();
    let token = owner_token(app).await;
    let mut db = app.db().await;
    let id = contract(&mut db).await;
    approve(app, &token, "sales_contract", id).await;
    let started = start(app, &token, "sales_contract", id).await;
    assert_eq!(started.status, 201, "{}", started.text());
    let doc_id = Uuid::parse_str(started.json()["sign_document"]["id"].as_str().unwrap()).unwrap();
    let signer_token: Uuid =
        sqlx::query_scalar("SELECT token FROM sign_signers WHERE document_id = $1")
            .bind(doc_id)
            .fetch_one(&mut db)
            .await
            .unwrap();

    let view = app
        .get("", &format!("/api/v1/sign/public/{signer_token}"))
        .await;
    assert_eq!(view.status, 200, "{}", view.text());
    let fields = view.json()["fields"].clone();
    assert_eq!(fields.as_array().unwrap().len(), 1);
    assert_eq!(fields[0]["field_type"], "signature");
    assert_eq!(view.json()["disclosure_version"], "v1");
    let pdf = app
        .get("", &format!("/api/v1/sign/public/{signer_token}/pdf"))
        .await;
    assert_eq!(pdf.status, 200);
    assert!(pdf.body.starts_with(b"%PDF"));

    let png = granary_services::later::verification::qr_png("pytest signature", 4, 1).unwrap();
    let sig = format!("data:image/png;base64,{}", b64(&png));
    let signed = app
        .post(
            "",
            &format!("/api/v1/sign/public/{signer_token}/sign"),
            json!({"signature_data": sig, "sign_reason": "Authorized by", "consent": true, "field_values": []}),
        )
        .await;
    assert_eq!(signed.status, 200, "{}", signed.text());
    assert_eq!(signed.json()["document_completed"], true);

    let row: (Option<String>, Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT status, signed_file_url, seal_level, sealed_at IS NOT NULL FROM sign_documents WHERE id = $1",
    )
    .bind(doc_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(row.0.as_deref(), Some("completed"));
    let signed_url = row.1.expect("the flattened, signed PDF was produced");
    assert_eq!(row.2.as_deref(), Some("pades-b-b"));
    assert!(row.3, "sealed");
    let (cstatus, cname, granary_id): (Option<String>, Option<String>, Option<Uuid>) = sqlx::query_as(
        "SELECT status, signed_by_customer_name, signed_by_company_id FROM contracts WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(cstatus.as_deref(), Some("signed"));
    assert!(cname.is_some());
    assert!(
        granary_id.is_some(),
        "the approving Owner is recorded as our signatory"
    );

    let (page, x, y, w, h): (Option<i32>, f64, f64, f64, f64) = sqlx::query_as(
        "SELECT page_number, x::float8, y::float8, width::float8, height::float8 FROM sign_fields WHERE document_id = $1",
    )
    .bind(doc_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let bytes = stored(&signed_url.replacen("signing/", "", 1)).unwrap();
    let report = granary_services::pdf_sign::validate_pdf(&bytes);
    assert_eq!(
        (report.intact, report.valid),
        (Some(true), Some(true)),
        "{report:?}"
    );
    // 3 Nov 2026: the verification issued for signing was sealed for sending (P=2) next to its master; the
    // e-sign flow signed the unsealed master, so its final seal is the usual P=1 certification
    assert_eq!(
        granary_services::pdf_sign::docmdp_permission(&bytes),
        Some(1)
    );
    let (master_key, sealed_key): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT pdf_object_key, sealed_object_key FROM document_verifications \
         WHERE entity_type = 'sales_contract' AND entity_id = $1 ORDER BY issued_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let master = stored(master_key.as_deref().unwrap()).unwrap();
    assert_eq!(granary_services::pdf_sign::docmdp_permission(&master), None);
    let outbound = stored(sealed_key.as_deref().expect("sealed at issue")).unwrap();
    assert_eq!(
        granary_services::pdf_sign::docmdp_permission(&outbound),
        Some(2)
    );
    let pdf = granary_services::pdftools::Document::open(&bytes).unwrap();
    let pno = page.unwrap_or(1) as usize - 1;
    let pr = pdf.page_rect(pno).unwrap();
    let bx = granary_services::pdftools::Rect::new(
        x / 100.0 * pr.width(),
        y / 100.0 * pr.height(),
        (x + w) / 100.0 * pr.width(),
        (y + h) / 100.0 * pr.height(),
    );
    let images = pdf.get_image_info(pno).unwrap();
    assert!(!images.is_empty(), "the signature image was stamped");
    assert!(images
        .iter()
        .all(|i| bx.contains(&i.bbox) || bx.intersects(&i.bbox)));

    // internal downloads of the completed document
    let internal = app
        .get(&token, &format!("/api/v1/sign/documents/{doc_id}/signed"))
        .await;
    assert_eq!(internal.status, 200);
    assert_eq!(internal.body, bytes);
    let fin = app
        .request(
            "POST",
            &format!("/api/v1/sign/documents/{doc_id}/finalize"),
            &token,
            None,
        )
        .await;
    assert_eq!(fin.status, 200, "{}", fin.text());
    assert_eq!(fin.json(), json!({"signed": true, "certificate": true}));
    let zip = app.get(&token, "/api/v1/sign/export").await;
    assert_eq!(zip.status, 200, "{}", zip.text());
    assert_eq!(
        zip.header("content-type").as_deref(),
        Some("application/zip")
    );
    assert!(zip.body.starts_with(b"PK\x03\x04"));
    let verify = app.get("", &format!("/api/v1/sign/verify/{doc_id}")).await;
    assert_eq!(verify.json()["digitally_sealed"], true);
    assert_eq!(verify.json()["seal_level_label"], "PAdES-B-B (baseline)");
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | ((*c.get(1).unwrap_or(&0) as u32) << 8)
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[tokio::test]
async fn test_signing_pdf_loads_when_the_title_has_non_latin_characters() {
    let app = app();
    let sa = app.superadmin().await;
    let key = format!("originals/{}.pdf", u("dash"));
    put(&key, anchors_pdf());
    let title = "Quotation QUO-2026-09-00007-1 \u{2014} Northwind Consulting \u{b7} \u{682a}\u{5f0f}\u{4f1a}\u{793e}";
    let mut db = app.db().await;
    let doc = Uuid::new_v4();
    let token = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sign_documents (id, title, status, original_file_url, is_deleted) VALUES ($1, $2, 'pending', $3, false)",
    )
    .bind(doc)
    .bind(title)
    .bind(format!("signing/{key}"))
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sign_signers (id, document_id, name, email, sign_order, token, status) \
         VALUES ($1, $2, 'Signer', 'signer@test.local', 1, $3, 'sent')",
    )
    .bind(Uuid::new_v4())
    .bind(doc)
    .bind(token)
    .execute(&mut db)
    .await
    .unwrap();
    let public = app
        .get("", &format!("/api/v1/sign/public/{token}/pdf"))
        .await;
    let internal = app
        .get(&sa.token, &format!("/api/v1/sign/documents/{doc}/pdf"))
        .await;
    for r in [&public, &internal] {
        assert_eq!(r.status, 200, "{}", r.text());
        assert!(r.body.starts_with(b"%PDF"));
        let h = r.header("content-disposition").unwrap();
        assert!(h.contains("filename*=UTF-8''"), "{h}");
    }
    assert_eq!(
        public.header("content-disposition").unwrap(),
        "inline; filename=\"Quotation QUO-2026-09-00007-1 _ Northwind Consulting _ ____.pdf\"; \
         filename*=UTF-8''Quotation%20QUO-2026-09-00007-1%20%E2%80%94%20Northwind%20Consulting%20%C2%B7%20%E6%A0%AA%E5%BC%8F%E4%BC%9A%E7%A4%BE.pdf"
    );
}

#[tokio::test]
async fn create_document_with_a_file_and_send_it() {
    let app = app();
    let owner = app.user_with(&["sign:manage"]).await;
    let form = reqwest::multipart::Form::new()
        .text("title", "Pytest upload")
        .part(
            "file",
            reqwest::multipart::Part::bytes(anchors_pdf()).file_name("a.pdf"),
        );
    let r = app
        .send(
            app.client()
                .post(app.url("/api/v1/sign/documents"))
                .bearer_auth(&owner.token)
                .multipart(form),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let doc = r.json();
    let id = doc["id"].as_str().unwrap();
    assert_eq!(
        doc["document_hash"].as_str().unwrap(),
        granary_compat::crypto::hashes::sha256_hex(&anchors_pdf())
    );
    assert_eq!(doc["file_url"], format!("/api/v1/sign/documents/{id}/pdf"));
    assert_eq!(
        stored(&format!("originals/{id}.pdf")).unwrap(),
        anchors_pdf()
    );

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"%PDF-1.4 v2".to_vec()).file_name("b.pdf"),
    );
    let r = app
        .send(
            app.client()
                .post(app.url(&format!("/api/v1/sign/documents/{id}/upload-pdf")))
                .bearer_auth(&owner.token)
                .multipart(form),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"document_hash": granary_compat::crypto::hashes::sha256_hex(b"%PDF-1.4 v2"),
               "file_url": format!("/api/v1/sign/documents/{id}/pdf")})
    );

    let r = app
        .request(
            "POST",
            &format!("/api/v1/sign/documents/{id}/send"),
            &owner.token,
            None,
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"detail": "Add at least one signer before sending"})
    );
    for (name, order, role) in [
        ("A", 1, "signer"),
        ("B", 1, "signer"),
        ("C", 2, "signer"),
        ("D", 1, "cc"),
    ] {
        let r = app
            .post(
                &owner.token,
                &format!("/api/v1/sign/documents/{id}/signers"),
                json!({"name": name, "email": format!("{name}@example.com"), "sign_order": order, "role": role}),
            )
            .await;
        assert_eq!(r.status, 201);
    }
    let r = app
        .request(
            "POST",
            &format!("/api/v1/sign/documents/{id}/send"),
            &owner.token,
            None,
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"status": "pending", "invitations_sent": 2})
    );
    let detail = app
        .get(&owner.token, &format!("/api/v1/sign/documents/{id}"))
        .await
        .json();
    let st: Vec<(String, String)> = detail["signers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().into(),
                s["status"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(st.iter().filter(|(_, s)| s == "sent").count(), 2, "{st:?}");
    let actions: Vec<&str> = detail["audit_logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        [
            "document_created",
            "document_sent",
            "sent_to_signer",
            "sent_to_signer"
        ]
    );
    let r = app
        .request(
            "POST",
            &format!("/api/v1/sign/documents/{id}/send"),
            &owner.token,
            None,
        )
        .await;
    assert_eq!(r.json(), json!({"detail": "Document is already pending"}));
}

// ─── Security stage 8 (M2, L2) ────────────────────────────────────────────────

/// M2: a signer linked to a user takes that user's name and email, whatever the request says.
#[tokio::test]
async fn a_signer_linked_to_a_user_gets_that_users_name_and_email() {
    let app = app();
    let owner = app.user_with(&["sign:manage"]).await;
    let victim = app
        .make_user(
            UserSpec::new()
                .name("Diana Prince")
                .email(&format!("{}@victim.test", u("diana"))),
        )
        .await;
    let mut db = app.db().await;
    let (doc, _, _) = sign_doc(&mut db, &[], None, Some(owner.id), "draft").await;
    let r = app
        .post(
            &owner.token,
            &format!("/api/v1/sign/documents/{doc}/signers"),
            json!({"name": "Robert Smith", "email": "attacker@evil.test", "user_id": victim.id.to_string(),
                   "signer_type": "internal"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let (name, email): (String, String) =
        sqlx::query_as("SELECT name, email FROM sign_signers WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        (name.as_str(), email),
        ("Diana Prince", victim.email.to_lowercase())
    );
    // an unknown user is refused
    let r = app
        .post(
            &owner.token,
            &format!("/api/v1/sign/documents/{doc}/signers"),
            json!({"name": "X", "email": "x@evil.test", "user_id": Uuid::new_v4().to_string()}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
}

/// M2: the public page offers a user's saved signature only to a signer with that user's email.
#[tokio::test]
async fn the_saved_signature_is_shown_only_to_the_same_person() {
    let app = app();
    let victim = app.make_user(UserSpec::new()).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET signature_data = 'data:image/png;base64,c2ln' WHERE id = $1")
        .bind(victim.id)
        .execute(&mut db)
        .await
        .unwrap();
    let (_, ids, tokens) = sign_doc(
        &mut db,
        &[(1, "sent"), (2, "pending")],
        None,
        None,
        "pending",
    )
    .await;
    // signer 0: linked to the victim but another address (an older row, or a direct write)
    // signer 1: linked to the victim with the victim's own address
    for (i, email) in [
        (0, "attacker@evil.test".to_string()),
        (1, victim.email.to_lowercase()),
    ] {
        sqlx::query("UPDATE sign_signers SET user_id = $2, email = $3 WHERE id = $1")
            .bind(ids[i])
            .bind(victim.id)
            .bind(email)
            .execute(&mut db)
            .await
            .unwrap();
    }
    let r = app
        .get("", &format!("/api/v1/sign/public/{}", tokens[0]))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["saved_signature"], Value::Null);
    let r = app
        .get("", &format!("/api/v1/sign/public/{}", tokens[1]))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json()["saved_signature"],
        json!("data:image/png;base64,c2ln")
    );
}

/// A document (`status`, deleted or not, `expires`) with a stored original PDF and certificate; its signer's token.
async fn stale_case(
    app: &TestApp,
    status: &str,
    deleted: bool,
    expires: Option<chrono::DateTime<Utc>>,
) -> String {
    let mut db = app.db().await;
    let (doc, _, tokens) = sign_doc(&mut db, &[(1, "signed")], expires, None, status).await;
    put(&format!("stale/{doc}.pdf"), b"%PDF-1.4 original".to_vec());
    put(&format!("stale/{doc}-cert.pdf"), b"%PDF-1.4 cert".to_vec());
    sqlx::query(
        "UPDATE sign_documents SET original_file_url = $2, certificate_url = $3, is_deleted = $4 WHERE id = $1",
    )
    .bind(doc)
    .bind(format!("signing/stale/{doc}.pdf"))
    .bind(format!("signing/stale/{doc}-cert.pdf"))
    .bind(deleted)
    .execute(&mut db)
    .await
    .unwrap();
    tokens[0].clone()
}

/// L2: the PDF and certificate links follow the signing page's rules: deleted, cancelled and expired
/// documents are not served.
#[tokio::test]
async fn stale_signing_links_serve_no_pdf_or_certificate() {
    let app = app();
    // the control: a live completed document serves both
    let live = stale_case(app, "completed", false, None).await;
    let r = app
        .get("", &format!("/api/v1/sign/public/{live}/pdf"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app
        .get("", &format!("/api/v1/sign/public/{live}/certificate"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let expired = Some(Utc::now() - Duration::hours(1));
    for (token, status) in [
        (stale_case(app, "cancelled", true, None).await, 400),
        (stale_case(app, "completed", true, None).await, 400),
        (stale_case(app, "draft", false, None).await, 400),
        (stale_case(app, "completed", false, expired).await, 410),
    ] {
        for what in ["pdf", "certificate"] {
            let r = app
                .get("", &format!("/api/v1/sign/public/{token}/{what}"))
                .await;
            assert_eq!(r.status, status, "{what}: {}", r.text());
        }
    }
}
