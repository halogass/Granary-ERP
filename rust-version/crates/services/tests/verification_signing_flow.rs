//! Port of backend/tests/test_sign_from_document.py (service level: `POST /sign/from-document` and
//! `/sign/documents/{id}/extend` are routers-wave wrappers around `start_from_document` /
//! `extend_deadline` / `status_for_document`) and test_audit_low_w2.py
//! `test_signature_reminder_audit_rows_survive_a_later_failure`. The template PDF the fake PDF service
//! returns is the one Python's test builds with PyMuPDF (anchors on the last page), recorded by
//! compat-tests/golden/gen_verification_delivery.py. The two public-signer-page tests exercise
//! routers/signing.py only (routers wave) and are not ported here.

mod verification_common;

use std::sync::{Arc, Mutex};

use chrono::{Duration, NaiveDate, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_services::approvals::engine;
use granary_services::verification::signing_flow::{self, seams, PendingInvitations};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;
use verification_common as c;

const TAG: &str = "verif_signing";

static INVITES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn anchors_pdf() -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../compat-tests/golden/verification_delivery/anchors.pdf"),
    )
    .unwrap()
}

async fn setup() -> (tokio::sync::MutexGuard<'static, ()>, PgConnection) {
    let g = c::serial().await;
    granary_services::approvals::notify::set_external_delivery_enabled(false);
    *c::PDF_BYTES.lock().unwrap() = anchors_pdf();
    INVITES.lock().unwrap().clear();
    seams::set_upload_to_minio(Some(Arc::new(|_data, name, _ct| {
        Box::pin(async move { Ok(format!("signing/{name}")) })
    })));
    seams::set_send_signing_invitation(Some(Arc::new(|d, s| {
        Box::pin(async move {
            INVITES.lock().unwrap().push((d, s));
            Ok(())
        })
    })));
    (g, c::session(TAG).await)
}

fn detail(e: &ApiError) -> Value {
    match e {
        ApiError::Http { detail, .. } => detail.clone(),
        other => panic!("not an HTTPException: {other:?}"),
    }
}

/// `_approve(token, entity_type, entity_id)`: submit, then approve when it is pending.
async fn approve(db: &mut PgConnection, user: &AuthUser, entity_type: &str, id: Uuid) {
    let req = engine::submit(db, entity_type, &id.to_string(), user, None, None, None)
        .await
        .unwrap();
    c::commit(db).await;
    if req.status == "pending" {
        let r = engine::act(db, &req.id.to_string(), user, "approved", None, None, None)
            .await
            .unwrap();
        assert_eq!(r.status, "approved");
        c::commit(db).await;
    }
}

async fn start(
    db: &mut PgConnection,
    user: &AuthUser,
    entity_type: &str,
    id: Uuid,
) -> Result<Value, ApiError> {
    let mut invites = PendingInvitations::default();
    let out = signing_flow::start_from_document(
        db,
        entity_type,
        &id.to_string(),
        user,
        None,
        &mut invites,
    )
    .await;
    match out {
        Ok(doc) => {
            c::commit(db).await;
            assert_eq!(invites.0.len(), 1);
            let st = signing_flow::status_for_document(db, entity_type, &id.to_string())
                .await
                .unwrap();
            assert_eq!(st["sign_document"]["id"], doc.id.to_string());
            Ok(st["sign_document"].clone())
        }
        Err(e) => {
            sqlx::query("ROLLBACK").execute(&mut *db).await.unwrap();
            sqlx::query("BEGIN").execute(&mut *db).await.unwrap();
            Err(e)
        }
    }
}

/// `_contract()`: a draft service contract for an organization with a legal email.
async fn contract(db: &mut PgConnection) -> Uuid {
    let email = format!("{}@customer.test", c::u("legal"));
    let org = c::make_org(db, &c::u("Pytest Customer"), Some(&email), None).await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contracts (id, number, customer_org_id, status, contract_type, value, currency, start_date, end_date, \
         auto_renew, is_deleted) VALUES ($1, $2, $3, 'draft', 'service', 250000000, 'JPY', '2026-10-01', '2027-09-30', false, false)",
    )
    .bind(id)
    .bind(c::u("PYT-CT"))
    .bind(org)
    .execute(&mut *db)
    .await
    .unwrap();
    c::commit(db).await;
    id
}

/// `_applicant()` and an offer letter for them (`POST /hr/offer-letters` with salary 15,000,000).
async fn offer_letter(db: &mut PgConnection) -> (Uuid, Uuid) {
    let opening = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO job_openings (id, title, department, employment_type, status, salary_currency, show_salary, is_remote, \
         is_deleted) VALUES ($1, $2, 'Engineering', 'full_time', 'open', 'JPY', false, false, false)",
    )
    .bind(opening)
    .bind(c::u("Pytest Engineer"))
    .execute(&mut *db)
    .await
    .unwrap();
    let applicant = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO applicants (id, job_opening_id, name, email, stage, is_deleted) VALUES ($1, $2, $3, $4, 'interview_2', false)",
    )
    .bind(applicant)
    .bind(opening)
    .bind(c::u("Pytest Candidate"))
    .bind(format!("{}@granary.test", c::u("cand")))
    .execute(&mut *db)
    .await
    .unwrap();
    let offer = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO offer_letters (id, number, applicant_id, job_opening_id, position_title, salary_gross_monthly, \
         salary_currency, probation_months, status, is_deleted) VALUES ($1, $2, $3, $4, 'Engineer', 15000000, 'JPY', 3, 'draft', false)",
    )
    .bind(offer)
    .bind(c::u("PYT-OL"))
    .bind(applicant)
    .bind(opening)
    .execute(&mut *db)
    .await
    .unwrap();
    c::commit(db).await;
    (offer, applicant)
}

#[tokio::test]
async fn test_unapproved_document_cannot_be_sent_for_signature() {
    let (_g, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let id = contract(&mut db).await;
    let e = start(&mut db, &owner, "sales_contract", id)
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 409);
    assert_eq!(detail(&e)["code"], "approval_required");
}

#[tokio::test]
async fn test_quotations_are_never_e_signed_the_approval_is_the_signature() {
    // Python approves the quotation first; the refusal comes from the policy's signature_flow, checked
    // before anything else, so the approval step (wave-3 blocked: nary.events.emit) is not needed here.
    let (_g, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let product = c::make_product(&mut db).await;
    let qid = c::make_quotation(&mut db, Some(product)).await;
    c::commit(&mut db).await;
    let e = start(&mut db, &owner, "quotation", qid).await.unwrap_err();
    assert_eq!(e.status().as_u16(), 400);
}

#[tokio::test]
async fn test_offer_letter_goes_to_the_candidate_and_completion_accepts_it() {
    let (_g, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let (offer, applicant) = offer_letter(&mut db).await;
    approve(&mut db, &owner, "offer_letter", offer).await;
    let doc = start(&mut db, &owner, "offer_letter", offer).await.unwrap();
    // Our approval is our signature: only the candidate signs, on their own block ([[SIG-2]]).
    let signers: Vec<(i64, String, String)> = doc["signers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["sign_order"].as_i64().unwrap(),
                s["signer_type"].as_str().unwrap().into(),
                s["status"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        signers,
        vec![(1, "external".to_string(), "sent".to_string())]
    );
    assert!(doc["expires_at"].is_string() && doc["can_extend"] == false); // offer letters allow no extension

    let doc_id = Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap();
    let mut sd: granary_db::rows::signing::SignDocument =
        sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
            .bind(doc_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let extend = signing_flow::extend_deadline(&mut db, &mut sd, 1, &owner)
        .await
        .unwrap_err();
    assert_eq!(extend.status().as_u16(), 400);

    sqlx::query("UPDATE sign_signers SET status = 'signed' WHERE document_id = $1")
        .bind(doc_id)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE sign_documents SET status = 'completed' WHERE id = $1")
        .bind(doc_id)
        .execute(&mut db)
        .await
        .unwrap();
    let sd: granary_db::rows::signing::SignDocument =
        sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
            .bind(doc_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let signers: Vec<granary_db::rows::signing::SignSigner> =
        sqlx::query_as("SELECT * FROM sign_signers WHERE document_id = $1")
            .bind(doc_id)
            .fetch_all(&mut db)
            .await
            .unwrap();
    signing_flow::on_sign_completed(&mut db, &sd, &signers)
        .await
        .unwrap();
    c::commit(&mut db).await;
    let (status, responded): (Option<String>, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("SELECT status, responded_at FROM offer_letters WHERE id = $1")
            .bind(offer)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(status.as_deref(), Some("accepted"));
    assert!(responded.is_some());
    let stage: Option<String> = sqlx::query_scalar("SELECT stage FROM applicants WHERE id = $1")
        .bind(applicant)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(stage.as_deref(), Some("offer_accepted"));
}

#[tokio::test]
async fn test_contract_deadline_can_be_extended_up_to_the_policy_cap() {
    let (_g, mut db) = setup().await;
    let owner = c::owner(&mut db).await;
    let id = contract(&mut db).await;
    approve(&mut db, &owner, "sales_contract", id).await;
    let doc = start(&mut db, &owner, "sales_contract", id).await.unwrap();
    let signers = doc["signers"].as_array().unwrap();
    assert_eq!(signers.len(), 1);
    assert!(signers[0]["email"]
        .as_str()
        .unwrap()
        .ends_with("@customer.test"));
    assert_eq!(doc["can_extend"], true);
    let doc_id = Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap();
    let (x, y): (Decimal, Decimal) =
        sqlx::query_as("SELECT x, y FROM sign_fields WHERE document_id = $1")
            .bind(doc_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    // on the counterparty's [[SIG-2]] anchor
    assert!(
        x > Decimal::from(52)
            && x < Decimal::from(56)
            && y > Decimal::from(80)
            && y < Decimal::from(84),
        "{x} {y}"
    );

    let mut sd: granary_db::rows::signing::SignDocument =
        sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
            .bind(doc_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let before = sd.expires_at.unwrap();
    signing_flow::extend_deadline(&mut db, &mut sd, 5, &owner)
        .await
        .unwrap();
    assert!(sd.expires_at.unwrap() > before);
    let too_far = signing_flow::extend_deadline(&mut db, &mut sd, 10, &owner)
        .await
        .unwrap_err();
    assert_eq!(too_far.status().as_u16(), 400);
    c::commit(&mut db).await;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM contracts WHERE id = $1")
        .bind(id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(status.as_deref(), Some("sent_for_signing"));
}

#[tokio::test]
async fn test_signature_reminder_audit_rows_survive_a_later_failure() {
    let (_g, mut db) = setup().await;
    let owner = c::make_person(&mut db, &[], &[], false).await;
    let doc = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sign_documents (id, title, status, source_type, expires_at, created_by_id, signature_level, jurisdictions, \
         is_deleted) VALUES ($1, $2, 'pending', 'standalone', $3, $4, 'ses', 'US,EU,ID', false)",
    )
    .bind(doc)
    .bind(c::u("Pytest sign"))
    .bind(Utc::now() + Duration::hours(2))
    .bind(owner.id)
    .execute(&mut db)
    .await
    .unwrap();
    for (i, (name, email)) in [("One", "one@granary.test"), ("Two", "two@granary.test")]
        .iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT INTO sign_signers (id, document_id, name, email, sign_order, role, token, status, signer_type) \
             VALUES ($1, $2, $3, $4, $5, 'signer', $6, 'sent', 'external')",
        )
        .bind(Uuid::new_v4())
        .bind(doc)
        .bind(name)
        .bind(email)
        .bind(i as i32 + 1)
        .bind(Uuid::new_v4())
        .execute(&mut db)
        .await
        .unwrap();
    }
    c::commit(&mut db).await;
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let c2 = calls.clone();
    seams::set_send_signing_invitation(Some(Arc::new(move |_d, s| {
        let calls = c2.clone();
        Box::pin(async move {
            let mut cs = calls.lock().unwrap();
            cs.push(s);
            if cs.len() == 2 {
                return Err(ApiError::internal(anyhow::anyhow!("mail server fell over")));
            }
            Ok(())
        })
    })));
    let r = signing_flow::remind_expiring(&mut db).await;
    assert!(r.is_err());
    sqlx::query("ROLLBACK").execute(&mut db).await.unwrap(); // the session ends with the error
    sqlx::query("BEGIN").execute(&mut db).await.unwrap();
    let rows: Vec<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT signer_id FROM sign_audit_logs WHERE document_id = $1 AND action = 'reminder_sent'",
    )
    .bind(doc)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.unwrap().to_string(), calls.lock().unwrap()[0]);
}

#[tokio::test]
async fn test_status_for_document_without_a_signature_request() {
    // status_for_document's two empty answers (no request, not a UUID).
    let (_g, mut db) = setup().await;
    let none =
        signing_flow::status_for_document(&mut db, "sales_contract", &Uuid::new_v4().to_string())
            .await
            .unwrap();
    assert_eq!(none, serde_json::json!({"sign_document": null}));
    let bad = signing_flow::status_for_document(&mut db, "sales_contract", "not-a-uuid")
        .await
        .unwrap();
    assert_eq!(bad, serde_json::json!({"sign_document": null}));
    let _ = NaiveDate::from_ymd_opt(2026, 1, 1);
}
