//! verification/service.rs, ported from backend/tests/test_verification.py (the service side of the
//! router tests: issue gate, public payload, revoke / reissue / supersede, change after approval, the offer
//! letter's login flags), test_audit_gaps_b.py / test_document_delivery.py (link expiry) and
//! test_catalog_pricing_c.py (printed quotation rows). The PDF sidecar and MinIO are wiremock fakes; one
//! async test function because the settings are process-wide.

mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_services::approvals::{adapters, engine};
use granary_services::verification::{render, service};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TS: &str = "2026-09-01T03:04:05+00:00";

async fn insert(db: &mut PgConnection, table: &str, row: Value) -> Uuid {
    let mut row = row.as_object().unwrap().clone();
    let id = row
        .entry("id")
        .or_insert_with(|| json!(Uuid::new_v4().to_string()))
        .as_str()
        .unwrap()
        .to_string();
    let cols = row
        .keys()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query(&format!("INSERT INTO {table} ({cols}) SELECT {cols} FROM json_populate_record(NULL::{table}, $1::json)"))
        .bind(Value::Object(row))
        .execute(&mut *db)
        .await
        .unwrap_or_else(|e| panic!("{table}: {e}"));
    Uuid::parse_str(&id).unwrap()
}

async fn person(db: &mut PgConnection, name: &str) -> AuthUser {
    let id = insert(
        db,
        "users",
        json!({"email": format!("{}@granary.test", Uuid::new_v4()), "name": name,
        "password_hash": "x", "is_active": true, "is_superadmin": false, "is_system_admin": false,
        "portal_type": "internal"}),
    )
    .await;
    sqlx::query_as("SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
                    is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, \
                    created_at, updated_at FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn make_quotation(db: &mut PgConnection, valid_until: &str) -> Uuid {
    let org = insert(
        db,
        "organizations",
        json!({"name": "Customer Ltd", "is_deleted": false}),
    )
    .await;
    let q = insert(db, "quotations", json!({"number": format!("Q-{}", &Uuid::new_v4().to_string()[..6]),
        "customer_org_id": org, "status": "draft", "currency": "JPY", "subtotal": "200000", "total": "200000",
        "issue_date": "2026-09-15", "valid_until": valid_until, "is_deleted": false, "created_at": TS, "updated_at": TS})).await;
    insert(
        db,
        "quotation_items",
        json!({"quotation_id": q, "name": "Laptop", "quantity": "2", "unit_price": "100000",
        "amount": "200000", "tax_pct": "0", "tax_amount": "0", "sort_order": 0}),
    )
    .await;
    q
}

/// An approved request whose fingerprint matches the document as it is now.
async fn approve(db: &mut PgConnection, entity_type: &str, id: Uuid, approver: &AuthUser) -> Uuid {
    let adapter = adapters::get_adapter(entity_type).unwrap();
    let entity = adapter.load(db, &id.to_string()).await.unwrap().unwrap();
    let facts = Value::Object(adapter.document_facts(db, &entity).await.unwrap());
    let fp = engine::facts_fingerprint(&facts, &[]);
    insert(db, "approval_requests", json!({"entity_type": entity_type, "entity_id": id, "status": "approved",
        "document_revision": adapter.revision(&entity), "submitted_at": TS, "completed_at": TS,
        "snapshot": {"fingerprint": fp}, "flags": [],
        "steps": [{"seq": 1, "name": "Sales lead", "decision": "approved", "decided_by_id": approver.id.to_string(),
                   "decided_at": "2026-09-15T08:00:00+00:00"}]})).await
}

async fn verification(
    db: &mut PgConnection,
    id: Uuid,
) -> granary_db::rows::verification::DocumentVerification {
    sqlx::query_as("SELECT * FROM document_verifications WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

fn detail_code(e: &ApiError) -> Option<String> {
    match e {
        ApiError::Http { detail, .. } => {
            detail.get("code").and_then(Value::as_str).map(String::from)
        }
        _ => None,
    }
}

fn status_of(e: &ApiError) -> u16 {
    e.status().as_u16()
}

#[test]
fn link_expiry_ends_the_day_in_the_given_zone() {
    // test_verification_link_expiry_ends_the_day_in_the_given_zone
    let now = Utc.with_ymd_and_hms(2026, 9, 19, 0, 0, 0).unwrap();
    let ends = NaiveDate::from_ymd_opt(2026, 10, 1);
    let tokyo = service::link_expiry_from(ends, Some(365), now, chrono_tz::Asia::Tokyo);
    assert_eq!(
        tokyo,
        Some(Utc.with_ymd_and_hms(2026, 10, 2, 14, 59, 59).unwrap())
    ); // 23:59:59 JST
    let london = service::link_expiry_from(ends, Some(365), now, chrono_tz::Europe::London);
    assert_eq!(
        london,
        Some(Utc.with_ymd_and_hms(2026, 10, 2, 22, 59, 59).unwrap())
    ); // 23:59:59 BST
       // test_document_delivery: no own end date -> the policy TTL; none -> no expiry
    assert_eq!(
        service::link_expiry_from(None, Some(30), now, chrono_tz::Asia::Tokyo),
        Some(now + chrono::Duration::days(30))
    );
    assert_eq!(
        service::link_expiry_from(None, None, now, chrono_tz::Asia::Tokyo),
        None
    );
    assert_eq!(
        service::link_expiry_from(None, Some(0), now, chrono_tz::Asia::Tokyo),
        None
    );
}

#[test]
fn render_helpers_match_the_web_formatter() {
    // test_render_helpers.py
    let html = "<ol style=\"--tw-border-spacing-y: 0; color: rgb(75, 85, 99);\"><li style=\"--tw-x: 0;\">\
                <span style=\"font-weight: bolder;\">Validity: </span>Valid for 14 days&nbsp;</li></ol>\
                <script>alert(1)</script><p onclick=\"x()\">Hi</p>\
                <a href=\"javascript:alert(1)\">bad</a> <a href=\"https://example.com\">ok</a>";
    let out = render::sanitize_html(Some(html));
    for bad in ["style", "--tw", "script", "alert", "onclick", "javascript"] {
        assert!(!out.contains(bad), "{bad} in {out}");
    }
    assert!(out.starts_with("<ol><li><strong>Validity: </strong>Valid for 14 days"));
    assert!(out.contains("<p>Hi</p>") && out.contains("<a href=\"https://example.com\">ok</a>"));
    assert_eq!(
        render::sanitize_html(Some("a < b & c\nnext")),
        "a &lt; b &amp; c<br>next"
    );
    assert_eq!(render::sanitize_html(None), "");
    assert_eq!(
        render::fmt_money(Some(38_473_062.into()), Some("JPY"), 0),
        "¥38,473,062"
    );
    assert_eq!(
        render::fmt_money(Some(rust_decimal::Decimal::new(12345, 1)), Some("USD"), 2),
        "$1,234.50"
    );
    assert_eq!(
        render::fmt_date_short(NaiveDate::from_ymd_opt(2026, 9, 15).unwrap()),
        "15 Sep 2026"
    );
    assert_eq!(
        render::sanitize_html(Some(
            "<p><span style=\"font-style: italic\">a</span><span class=\"x\">b</span></p>"
        )),
        "<p><em>a</em><span>b</span></p>"
    );
}

#[tokio::test]
async fn verification_lifecycle() {
    let pdf = MockServer::start().await;
    let minio = MockServer::start().await;
    std::env::set_var("PDF_SERVICE_URL", pdf.uri());
    std::env::set_var("MINIO_ENDPOINT", minio.uri().trim_start_matches("http://"));
    std::env::set_var("APP_URL", "https://erp.example/");
    let pool = support::fresh_db("s11_verification").await;
    Mock::given(method("POST"))
        .and(path("/render"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.4 document".to_vec()))
        .mount(&pdf)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/granary-documents"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&minio)
        .await;
    Mock::given(method("PUT"))
        .and(path("/granary-documents"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&minio)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/granary-documents/verification/.*\\.pdf$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&minio)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/granary-documents/verification/.*\\.pdf$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.4 document".to_vec()))
        .mount(&minio)
        .await;

    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let issuer = person(db, "Issuer").await;

    // test_unapproved_documents_cannot_be_issued
    let qid = make_quotation(db, "2027-10-01").await;
    let err = service::issue(db, "quotation", &qid.to_string(), &issuer)
        .await
        .unwrap_err();
    assert_eq!(status_of(&err), 409);
    assert_eq!(detail_code(&err).as_deref(), Some("approval_required"));

    // test_issued_quotation_verifies_publicly_and_serves_the_pdf
    approve(db, "quotation", qid, &issuer).await;
    let v = service::issue(db, "quotation", &qid.to_string(), &issuer)
        .await
        .unwrap();
    let d = service::internal_dict(&v, true, Some("valid"));
    assert_eq!(d["status"], "active");
    assert!(d["url"]
        .as_str()
        .unwrap()
        .ends_with(&format!("/v/{}", d["token"].as_str().unwrap())));
    assert_eq!(
        d["url"].as_str().unwrap(),
        format!("https://erp.example/v/{}", v.token)
    );
    assert_eq!(d["allow_download"], true);
    assert_eq!(v.short_code.len(), 10);
    assert!(v
        .short_code
        .chars()
        .all(|c| service::CODE_ALPHABET.contains(c)));
    assert_eq!(
        v.document_hash.as_deref(),
        Some(hex::encode(sha2::Sha256::digest(b"%PDF-1.4 document")).as_str())
    );
    assert_eq!(
        v.pdf_object_key.as_deref(),
        Some(format!("verification/quotation/{}.pdf", v.id).as_str())
    );
    // valid until 1 Oct 2027 -> the link lives to the end of 2 Oct, company time (Tokyo)
    assert_eq!(
        v.expires_at,
        Some(Utc.with_ymd_and_hms(2027, 10, 2, 14, 59, 59).unwrap())
    );
    let mut v = verification(db, v.id).await;
    let state = service::evaluate(db, &mut v).await.unwrap();
    assert_eq!(state, "valid");
    let body = service::public_payload(db, &v, &state).await.unwrap();
    assert_eq!(body["state"], "valid");
    assert_eq!(body["can_view_document"], true);
    let labels: Vec<&str> = body["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["label"].as_str().unwrap())
        .collect();
    assert!(
        labels.contains(&"Document") && labels.contains(&"Total"),
        "{labels:?}"
    );
    assert!(
        !body["approvals"].as_array().unwrap().is_empty(),
        "the approval record is shown"
    );
    assert_eq!(body["approvals"][0]["name"], "Issuer");
    assert_eq!(
        service::get_pdf(v.pdf_object_key.as_deref().unwrap())
            .await
            .unwrap(),
        b"%PDF-1.4 document"
    );
    // the sidecar got the QR footer
    let reqs = pdf.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs.last().unwrap().body).unwrap();
    assert!(sent["footer"]["qr_data_uri"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    assert_eq!(
        sent["footer"]["code"],
        format!("Verification code {}", v.short_code)
    );
    assert_eq!(sent["template"], "document");

    // test_revoked_and_reissued_codes_stop_verifying
    let first = v;
    let second = service::issue(db, "quotation", &qid.to_string(), &issuer)
        .await
        .unwrap();
    let mut first = verification(db, first.id).await;
    assert_eq!(first.status, "superseded");
    assert_eq!(first.superseded_by_id, Some(second.id));
    let mut second = verification(db, second.id).await;
    service::revoke(db, &mut second, &issuer, "Sent to the wrong customer")
        .await
        .unwrap();
    let again = service::revoke(db, &mut second, &issuer, "Twice")
        .await
        .unwrap_err();
    assert_eq!(status_of(&again), 409);
    assert_eq!(
        service::evaluate(db, &mut first).await.unwrap(),
        "superseded"
    );
    let mut second = verification(db, second.id).await;
    assert_eq!(
        second.revoke_reason.as_deref(),
        Some("Sent to the wrong customer")
    );
    assert_eq!(service::evaluate(db, &mut second).await.unwrap(), "revoked");

    // test_changing_the_document_after_issue_invalidates_the_code
    let third = service::issue(db, "quotation", &qid.to_string(), &issuer)
        .await
        .unwrap();
    sqlx::query("UPDATE quotation_items SET unit_price = 1 WHERE quotation_id = $1")
        .bind(qid)
        .execute(&mut *db)
        .await
        .unwrap();
    let mut third = verification(db, third.id).await;
    assert_eq!(
        service::evaluate(db, &mut third).await.unwrap(),
        "not_approved"
    );

    // test_superseding_the_approval_supersedes_the_code
    let q2 = make_quotation(db, "2027-10-01").await;
    approve(db, "quotation", q2, &issuer).await;
    let v2 = service::issue(db, "quotation", &q2.to_string(), &issuer)
        .await
        .unwrap();
    service::supersede_for_entity(db, "quotation", q2)
        .await
        .unwrap();
    let mut v2 = verification(db, v2.id).await;
    assert_eq!(service::evaluate(db, &mut v2).await.unwrap(), "superseded");

    // expiry: an active code past its expiry turns expired
    let q3 = make_quotation(db, "2027-10-01").await;
    approve(db, "quotation", q3, &issuer).await;
    let v3 = service::issue(db, "quotation", &q3.to_string(), &issuer)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE document_verifications SET expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(v3.id)
    .execute(&mut *db)
    .await
    .unwrap();
    let mut v3 = verification(db, v3.id).await;
    assert_eq!(service::evaluate(db, &mut v3).await.unwrap(), "expired");
    assert_eq!(verification(db, v3.id).await.status, "expired");

    // a cancelled document cannot be issued, and its codes read "void"
    sqlx::query("UPDATE quotations SET status = 'cancelled' WHERE id = $1")
        .bind(q3)
        .execute(&mut *db)
        .await
        .unwrap();
    let err = service::issue(db, "quotation", &q3.to_string(), &issuer)
        .await
        .unwrap_err();
    assert_eq!(status_of(&err), 400);
    let q4 = make_quotation(db, "2027-10-01").await;
    approve(db, "quotation", q4, &issuer).await;
    let v4 = service::issue(db, "quotation", &q4.to_string(), &issuer)
        .await
        .unwrap();
    sqlx::query("UPDATE quotations SET status = 'cancelled' WHERE id = $1")
        .bind(q4)
        .execute(&mut *db)
        .await
        .unwrap();
    let mut v4 = verification(db, v4.id).await;
    assert_eq!(service::evaluate(db, &mut v4).await.unwrap(), "void");

    // unsupported type / missing document / QR disabled
    let err = service::issue(db, "expense", &Uuid::new_v4().to_string(), &issuer)
        .await
        .unwrap_err();
    assert_eq!(
        (status_of(&err), err.body()["detail"].clone()),
        (
            400,
            json!("Expense claim documents are not issued with a verification QR")
        )
    );
    let err = service::issue(db, "quotation", &Uuid::new_v4().to_string(), &issuer)
        .await
        .unwrap_err();
    assert_eq!(
        (status_of(&err), err.body()["detail"].clone()),
        (404, json!("Quotation not found"))
    );
    sqlx::query("UPDATE approval_policies SET qr_enabled = false WHERE entity_type = 'quotation'")
        .execute(&mut *db)
        .await
        .unwrap();
    let err = service::issue(db, "quotation", &q2.to_string(), &issuer)
        .await
        .unwrap_err();
    assert_eq!(
        err.body()["detail"],
        json!("QR verification is not enabled for quotations")
    );
    sqlx::query("UPDATE approval_policies SET qr_enabled = true WHERE entity_type = 'quotation'")
        .execute(&mut *db)
        .await
        .unwrap();

    // test_offer_letter_code_requires_an_hr_login: the policy's login and download flags are copied
    let job = insert(
        db,
        "job_openings",
        json!({"title": "Engineer", "is_deleted": false, "created_at": TS, "updated_at": TS}),
    )
    .await;
    let applicant = insert(
        db,
        "applicants",
        json!({"job_opening_id": job, "name": "Rachel", "is_deleted": false,
        "created_at": TS, "updated_at": TS}),
    )
    .await;
    let offer = insert(db, "offer_letters", json!({"number": "OL-1", "applicant_id": applicant, "position_title": "Engineer",
        "salary_gross_monthly": "15000000", "salary_currency": "JPY", "status": "draft", "is_deleted": false,
        "created_at": TS, "updated_at": TS})).await;
    approve(db, "offer_letter", offer, &issuer).await;
    let ov = service::issue(db, "offer_letter", &offer.to_string(), &issuer)
        .await
        .unwrap();
    assert!(ov.requires_login && !ov.allow_download);
    let mut ov = verification(db, ov.id).await;
    let state = service::evaluate(db, &mut ov).await.unwrap();
    assert_eq!(state, "valid");
    let body = service::public_payload(db, &ov, &state).await.unwrap();
    assert!(!body["facts"].as_array().unwrap().iter().any(|f| f["label"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("salary")));
    assert_eq!(body["allow_download"], false);
}

#[tokio::test]
async fn rendered_quotation_rows_carry_sku_addon_indent_and_term() {
    // test_catalog_pricing_c.py::test_rendered_quotation_rows_carry_sku_addon_indent_and_term
    let pool = support::fresh_db("s11_render_rows").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let org = insert(
        db,
        "organizations",
        json!({"name": "Org", "is_deleted": false}),
    )
    .await;
    let product = insert(db, "product_catalog", json!({"sku": "SUB-1", "name": "Sub", "product_type": "saas",
        "sale_model": "recurring", "price_metric": "seat", "pricing_mode": "list", "is_whitelabel": false,
        "is_deleted": false, "terms_note": "Covers updates and support"})).await;
    let q = insert(db, "quotations", json!({"number": "Q-ROWS", "customer_org_id": org, "status": "draft", "currency": "JPY",
        "subtotal": "15600013", "tax_amount": "1716000", "total": "17316013", "issue_date": "2026-09-15",
        "valid_until": "2026-10-15", "is_deleted": false, "created_at": TS, "updated_at": TS,
        "tax_lines": [{"tax_id": null, "code": "VAT", "pct": 11, "base": 15600000, "amount": 1716000}]})).await;
    let first = insert(db, "quotation_items", json!({"quotation_id": q, "product_id": product, "sku": "SUB-1",
        "name": "Subscription", "quantity": "13", "unit_price": "100000", "amount": "15600000", "sale_model": "recurring",
        "price_metric": "seat", "rate_period_months": 1, "billing_interval_months": 12, "term_months": 12,
        "tax_pct": "11", "tax_amount": "1716000", "sort_order": 0})).await;
    insert(db, "quotation_items", json!({"quotation_id": q, "sku": "CARE-X", "name": "Care", "quantity": "13",
        "unit_price": "1", "amount": "13", "parent_item_id": first, "tax_pct": "0", "tax_amount": "0", "sort_order": 1})).await;
    let entity = render::load(db, "quotation", &q.to_string())
        .await
        .unwrap()
        .unwrap();
    let adapters::Entity::Quotation(doc) = &entity else {
        panic!()
    };
    let company = render::company_block(db).await.unwrap();
    let (_, data, _) = render::quotation(db, doc, &company).await.unwrap();
    let rows = data["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0]["sku"], "SUB-1");
    assert_eq!(
        rows[0][0]["term"],
        "per seat / month · 12 months, billed annually"
    );
    assert_eq!(rows[0][0]["note"], "Covers updates and support");
    assert_eq!(rows[0][4]["text"], "11%");
    assert_eq!(rows[1][0]["indent"], true);
    assert!(rows[1][0]["title"].as_str().unwrap().starts_with("↳ "));
    assert!(data["totals"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["label"] == "VAT 11%"));
}

use sha2::Digest as _;
