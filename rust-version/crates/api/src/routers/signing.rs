//! backend/app/routers/signing.py, mounted at `/api/v1/sign` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (internal routes use `InternalUser`, the signer and
//! verification routes are public).
//!
//! Port notes
//! - Ids Python compares to a UUID column without `parse_uuid` (`SignDocument.id == doc_id`,
//!   `SignSigner.token == token`) reach asyncpg as strings: a malformed one is a DataError, the central
//!   422. [`pg_uuid`] reproduces asyncpg's acceptance (32..36 characters, hex digits and dashes).
//! - `_upload_to_minio` / `_send_signing_invitation` and the PDF/QR/ZIP helpers that need MuPDF, image,
//!   qrcode or flate2 live in granary_services::later::verification (signing_flow calls the first two too).
//! - Invitation emails Python starts with `asyncio.create_task` before the commit are spawned after it
//!   (they read rows that were already committed, so the email is the same).
//! - A new PAdES seal cannot be byte-identical to pyHanko's output (fresh signing time, signature value);
//!   tests validate it instead (pdf_sign::validate_pdf).

mod certificate;

pub use certificate::{build_certificate_html, legal_basis_for, py_title};

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, InternalUser};
use granary_core::config::settings;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::signing::{SignAuditLog, SignDocument, SignField, SignSigner};
use granary_services::later::verification as helpers;

use super::at;
use crate::extract::{parse_uuid, ClientIp, PyForm};
use crate::resp::{self, iso, iso_opt};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/sign";

/// The routes of `router` in signing.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/documents"),
            get(list_documents).post(create_document),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/upload-pdf"),
            post(upload_pdf),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}"),
            get(get_document).delete(cancel_document),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/pdf"),
            get(get_document_pdf),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/signed"),
            get(download_signed_pdf),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/certificate"),
            get(download_certificate_internal),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/finalize"),
            post(finalize_document),
        )
        .route(&at(PREFIX, "/export"), get(download_all))
        .route(&at(PREFIX, "/signers/search"), get(search_signers))
        .route(&at(PREFIX, "/documents/{doc_id}/signers"), post(add_signer))
        .route(
            &at(PREFIX, "/documents/{doc_id}/signers/{signer_id}"),
            delete(remove_signer),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/self-sign"),
            post(self_sign),
        )
        .route(&at(PREFIX, "/documents/{doc_id}/fields"), post(add_field))
        .route(
            &at(PREFIX, "/documents/{doc_id}/fields/batch"),
            post(add_fields_batch),
        )
        .route(&at(PREFIX, "/documents/{doc_id}/send"), post(send_document))
        .route(
            &at(PREFIX, "/documents/{doc_id}/remind"),
            post(remind_signer),
        )
        .route(&at(PREFIX, "/public/{token}"), get(public_view_document))
        .route(&at(PREFIX, "/public/{token}/pdf"), get(proxy_pdf))
        .route(
            &at(PREFIX, "/public/{token}/certificate"),
            get(public_download_certificate),
        )
        .route(&at(PREFIX, "/public/{token}/sign"), post(submit_signature))
        .route(
            &at(PREFIX, "/public/{token}/decline"),
            post(decline_signing),
        )
        .route(&at(PREFIX, "/verify/{document_id}"), get(verify_document))
}

// ─── Compliance constants ────────────────────────────────────────────────────

pub const ESIGN_DISCLOSURE_VERSION: &str = "v1";
pub const ESIGN_DISCLOSURE: &str = concat!(
    "ELECTRONIC RECORDS AND SIGNATURE DISCLOSURE & CONSENT\n\n",
    "By signing this document electronically you agree to conduct this transaction by electronic ",
    "means and that your electronic signature is legally binding, with the same force and effect as ",
    "a handwritten signature, under the U.S. ESIGN Act (15 U.S.C. ch. 96) and UETA, and the EU eIDAS ",
    "Regulation (EU) 910/2014.\n\n",
    "1. Consent to electronic records. You consent to receive and sign records relating to this ",
    "transaction electronically rather than on paper.\n",
    "2. Right to paper copies. You may request a paper copy of any signed record at no charge by ",
    "contacting the sending party.\n",
    "3. Withdrawing consent. You may decline to sign electronically; to do so, do not complete this ",
    "signature and notify the sending party. Withdrawal does not affect records already signed.\n",
    "4. Hardware/software. To access and retain these records you need a current web browser, an ",
    "internet connection, a device able to view PDF files, and a valid email address.\n",
    "5. Keeping your details current. Ensure the sending party has your correct email address.\n",
    "6. Identity & integrity. Your signature is linked to you via the unique signing link sent to ",
    "your email and the audit trail recorded (timestamp, IP address). The completed document is ",
    "sealed with a tamper-evident cryptographic signature; any later change is detectable."
);

const SIGNATURE_DATA_PREFIX: &str = "data:image/png;base64,";
const SIGNATURE_MAX_BYTES_KEY: &str = "sign.signature_max_bytes";
const SIGN_REASONS: [&str; 4] = ["Approved by", "Authored by", "Authorized by", "Reviewed by"];
const EM: &str = ":";

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// `_require_sign_view(user)`.
pub fn require_sign_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !["sign:view", "sign:manage", "sign:send"]
        .iter()
        .any(|p| user.perms.contains(*p))
    {
        return Err(ApiError::http(403, "Permission required: sign:view"));
    }
    Ok(())
}

/// `_require_sign_manage(user)`.
pub fn require_sign_manage(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !["sign:manage", "sign:send"]
        .iter()
        .any(|p| user.perms.contains(*p))
    {
        return Err(ApiError::http(403, "Permission required: sign:manage"));
    }
    Ok(())
}

fn now_micro() -> DateTime<Utc> {
    let n = Utc::now();
    DateTime::from_timestamp_micros(n.timestamp_micros()).unwrap_or(n)
}

fn is_viewer(s: &SignSigner, viewer: Option<&AuthUser>) -> bool {
    let Some(viewer) = viewer else { return false };
    if s.user_id.is_some() && s.user_id == Some(viewer.id) {
        return true;
    }
    !s.email.is_empty()
        && !viewer.email.is_empty()
        && s.email.to_lowercase() == viewer.email.to_lowercase()
}

/// `_signer_dict(s, viewer)`: the token only for that signer.
pub fn signer_dict(s: &SignSigner, viewer: Option<&AuthUser>) -> Value {
    json!({
        "id": s.id.to_string(),
        "document_id": s.document_id.to_string(),
        "name": s.name,
        "email": s.email,
        "sign_order": s.sign_order,
        "role": s.role,
        "signer_type": s.signer_type,
        "partner_account_id": s.partner_account_id.map(|p| p.to_string()),
        "token": if is_viewer(s, viewer) { Value::String(s.token.to_string()) } else { Value::Null },
        "status": s.status,
        "sent_at": iso_opt(s.sent_at.as_ref()),
        "viewed_at": iso_opt(s.viewed_at.as_ref()),
        "signed_at": iso_opt(s.signed_at.as_ref()),
        "declined_reason": s.declined_reason,
        "has_signature": s.signature_data.as_deref().is_some_and(|d| !d.is_empty()),
    })
}

fn dec_f(d: Option<&Decimal>) -> Value {
    d.map(resp::dec_float).unwrap_or(Value::Null)
}

/// `_field_dict(f)`.
pub fn field_dict(f: &SignField) -> Value {
    json!({
        "id": f.id.to_string(),
        "document_id": f.document_id.to_string(),
        "signer_id": f.signer_id.map(|s| s.to_string()),
        "field_type": f.field_type,
        "page_number": f.page_number,
        "x": dec_f(f.x.as_ref()),
        "y": dec_f(f.y.as_ref()),
        "width": dec_f(f.width.as_ref()),
        "height": dec_f(f.height.as_ref()),
        "is_required": f.is_required,
        "label": f.label,
        "value": f.value,
    })
}

fn log_dict(l: &SignAuditLog) -> Value {
    json!({
        "id": l.id.to_string(),
        "action": l.action,
        "description": l.description,
        "signer_id": l.signer_id.map(|s| s.to_string()),
        "ip_address": l.ip_address,
        "created_at": iso(&l.created_at),
    })
}

/// A document with what `_doc_dict` reads (`include_detail` adds fields and audit logs).
struct Loaded {
    doc: SignDocument,
    signers: Vec<SignSigner>,
    fields: Vec<SignField>,
    logs: Vec<SignAuditLog>,
}

fn doc_dict(
    doc: &SignDocument,
    signers: &[SignSigner],
    detail: Option<(&[SignField], &[SignAuditLog])>,
    viewer: Option<&AuthUser>,
) -> Value {
    let mut d = json!({
        "id": doc.id.to_string(),
        "title": doc.title,
        "status": doc.status,
        "source_type": doc.source_type,
        "source_id": doc.source_id.map(|s| s.to_string()),
        "document_hash": doc.document_hash,
        "certificate_url": doc.certificate_url,
        "file_url": if doc.original_file_url.as_deref().is_some_and(|u| !u.is_empty()) {
            Value::String(format!("/api/v1/sign/documents/{}/pdf", doc.id))
        } else { Value::Null },
        "certificate_generated_at": iso_opt(doc.certificate_generated_at.as_ref()),
        "sealed_at": iso_opt(doc.sealed_at.as_ref()),
        "seal_fingerprint": doc.seal_fingerprint,
        "signature_level": doc.signature_level.as_deref().filter(|s| !s.is_empty()).unwrap_or("ses"),
        "seal_level": doc.seal_level,
        "message": doc.message,
        "expires_at": iso_opt(doc.expires_at.as_ref()),
        "completed_at": iso_opt(doc.completed_at.as_ref()),
        "created_at": iso(&doc.created_at),
        "signer_count": signers.len(),
        "signed_count": signers.iter().filter(|s| s.status.as_deref() == Some("signed")).count(),
    });
    if let Some((fields, logs)) = detail {
        let obj = d.as_object_mut().expect("object");
        obj.insert(
            "signers".into(),
            Value::Array(signers.iter().map(|s| signer_dict(s, viewer)).collect()),
        );
        obj.insert(
            "fields".into(),
            Value::Array(fields.iter().map(field_dict).collect()),
        );
        obj.insert(
            "audit_logs".into(),
            Value::Array(logs.iter().map(log_dict).collect()),
        );
    }
    d
}

/// `_add_audit(db, document_id, action, description, signer_id, ip_address, user_agent)` (flushed at once).
async fn add_audit(
    conn: &mut PgConnection,
    document_id: Uuid,
    action: &str,
    description: Option<&str>,
    signer_id: Option<Uuid>,
    ip_address: Option<&str>,
    user_agent: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO sign_audit_logs (id, document_id, signer_id, action, description, ip_address, user_agent) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind(signer_id)
    .bind(action)
    .bind(description)
    .bind(ip_address)
    .bind(user_agent)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `doc.signers` (relationship ordered by sign_order).
async fn signers_of(conn: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<SignSigner>> {
    Ok(sqlx::query_as(
        "SELECT * FROM sign_signers WHERE sign_signers.document_id IN ($1) ORDER BY sign_signers.sign_order",
    )
    .bind(doc_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// `doc.fields` (no order: the table's).
async fn fields_of(conn: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<SignField>> {
    Ok(
        sqlx::query_as("SELECT * FROM sign_fields WHERE sign_fields.document_id IN ($1)")
            .bind(doc_id)
            .fetch_all(&mut *conn)
            .await?,
    )
}

/// `doc.audit_logs` (ordered by created_at).
async fn logs_of(conn: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<SignAuditLog>> {
    Ok(sqlx::query_as(
        "SELECT * FROM sign_audit_logs WHERE sign_audit_logs.document_id IN ($1) ORDER BY sign_audit_logs.created_at",
    )
    .bind(doc_id)
    .fetch_all(&mut *conn)
    .await?)
}

async fn load_full(conn: &mut PgConnection, doc: SignDocument) -> ApiResult<Loaded> {
    let signers = signers_of(conn, doc.id).await?;
    let fields = fields_of(conn, doc.id).await?;
    let logs = logs_of(conn, doc.id).await?;
    Ok(Loaded {
        doc,
        signers,
        fields,
        logs,
    })
}

/// `_load_owned_doc(doc_id, current_user, db)`: the document (signers and audit logs are loaded by the
/// callers that need them).
async fn load_owned_doc(
    conn: &mut PgConnection,
    doc_id: &str,
    user: &AuthUser,
) -> ApiResult<SignDocument> {
    let id = parse_uuid(doc_id, "doc_id")?;
    let doc: Option<SignDocument> =
        sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(doc) = doc else {
        return Err(ApiError::http(404, "Document not found"));
    };
    if !user.is_superadmin && doc.created_by_id != Some(user.id) {
        return Err(ApiError::http(403, "Access denied"));
    }
    Ok(doc)
}

/// `_content_disposition(disposition, filename)`: an ASCII fallback plus the exact UTF-8 name (RFC 5987).
pub fn content_disposition(disposition: &str, filename: &str) -> String {
    let fallback: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii() && (c.is_ascii_alphanumeric() || " -_.".contains(c)) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fallback = py_strip(&fallback);
    let fallback = if fallback.is_empty() {
        "document"
    } else {
        fallback
    };
    format!(
        "{disposition}; filename=\"{fallback}\"; filename*=UTF-8''{}",
        quote_all(filename)
    )
}

/// `urllib.parse.quote(s, safe='')`.
fn quote_all(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(b) {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `"".join(c if c.isalnum() or c in " -_" else "_" for c in title)`.
fn underscore_title(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `_safe_title(title)`.
fn safe_title(title: &str) -> String {
    let t = if title.is_empty() { "document" } else { title };
    let s = underscore_title(t);
    let s = py_strip(&s);
    if s.is_empty() {
        "document".into()
    } else {
        s.to_string()
    }
}

fn store_key(file_url: &str) -> String {
    file_url.replacen("signing/", "", 1)
}

/// `_minio_bytes(file_url)`.
async fn minio_bytes(file_url: &str) -> Result<Vec<u8>, granary_services::storage::StoreError> {
    let store = granary_services::storage::minio();
    store
        .get_object(&settings().MINIO_BUCKET_DOCUMENTS, &store_key(file_url))
        .await
}

fn pdf_response(data: Vec<u8>, disposition: String) -> Response {
    resp::with_headers(
        resp::bytes_response(200, data, "application/pdf"),
        &[("content-disposition", disposition)],
    )
}

/// `request.headers.get("user-agent", "")[:500]`.
fn user_agent(headers: &axum::http::HeaderMap) -> String {
    let raw: String = headers
        .get("user-agent")
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
        .unwrap_or_default();
    raw.chars().take(500).collect()
}

// ─── Certificate / signed PDF generation ─────────────────────────────────────

/// `_generate_qr_base64(url)` ("" on failure).
fn generate_qr_base64(url: &str) -> String {
    use base64_lite::encode;
    match helpers::qr_png(url, 8, 2) {
        Ok(png) => encode(&png),
        Err(_) => String::new(),
    }
}

mod base64_lite {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    /// `base64.b64encode(data).decode()`.
    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }
}

/// `_generate_certificate(doc, app_url, db)`: renders the certificate HTML with the PDF service and stores
/// it; None on any failure (logged).
async fn generate_certificate(
    doc: &SignDocument,
    signers: &[SignSigner],
    logs: &[SignAuditLog],
) -> Option<String> {
    let s = settings();
    let verification_url = format!("{}/verify/{}", s.APP_URL, doc.id);
    let html = build_certificate_html(
        doc,
        signers,
        logs,
        Utc::now(),
        &generate_qr_base64(&verification_url),
        &s.SIGN_SEAL_NAME,
    );
    let result: anyhow::Result<Option<String>> = async {
        let resp = granary_services::http::client()
            .post(format!("{}/pdf", s.PDF_SERVICE_URL))
            .timeout(std::time::Duration::from_secs(30))
            .bearer_auth(&s.PDF_SERVICE_TOKEN)
            .json(
                &json!({"html": html, "options": {"format": "A4", "margin": {
                "top": "10mm", "bottom": "10mm", "left": "10mm", "right": "10mm"}}}),
            )
            .send()
            .await?;
        if resp.status().as_u16() == 200 {
            let pdf_bytes = resp.bytes().await?.to_vec();
            let object_name = format!("certificates/{}.pdf", doc.id);
            helpers::routers_signing_upload_to_minio(
                pdf_bytes,
                object_name.clone(),
                "application/pdf".into(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            return Ok(Some(format!("signing/{object_name}")));
        }
        Ok(None)
    }
    .await;
    match result {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "certificate_generation_failed");
            None
        }
    }
}

/// `_generate_signed_pdf(doc, db)`: flattens the signature stamps and field values onto the original,
/// applies the PAdES seal (falling back to the unsealed PDF) and stores it. The seal attributes Python
/// sets on `doc` are written to the row and to `doc`. None on failure (logged).
async fn generate_signed_pdf(conn: &mut PgConnection, doc: &mut SignDocument) -> Option<String> {
    let original_url = doc.original_file_url.clone().filter(|u| !u.is_empty())?;
    match generate_signed_pdf_inner(conn, doc, &original_url).await {
        Ok(url) => Some(url),
        Err(e) => {
            tracing::error!(error = %e, doc_id = %doc.id, "signed_pdf_generation_failed");
            None
        }
    }
}

async fn generate_signed_pdf_inner(
    conn: &mut PgConnection,
    doc: &mut SignDocument,
    original_url: &str,
) -> anyhow::Result<String> {
    let tz = granary_core::clock::local_tz(&mut *conn)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let original = minio_bytes(original_url).await?;
    let fields: Vec<SignField> =
        sqlx::query_as("SELECT * FROM sign_fields WHERE sign_fields.document_id = $1")
            .bind(doc.id)
            .fetch_all(&mut *conn)
            .await?;
    // selectinload(SignField.signer)
    let ids: Vec<Uuid> = fields.iter().filter_map(|f| f.signer_id).collect();
    let signers: Vec<SignSigner> = if ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as("SELECT * FROM sign_signers WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let stamp_fields: Vec<helpers::StampField> = fields
        .iter()
        .map(|f| helpers::StampField {
            field_type: f.field_type.clone(),
            page_number: f.page_number,
            x: f.x.and_then(|d| d.to_f64()),
            y: f.y.and_then(|d| d.to_f64()),
            width: f.width.and_then(|d| d.to_f64()),
            height: f.height.and_then(|d| d.to_f64()),
            value: f.value.clone(),
            signer: f
                .signer_id
                .and_then(|sid| signers.iter().find(|s| s.id == sid).cloned()),
        })
        .collect();
    let verify_url = format!("{}/verify/{}", settings().APP_URL, doc.id);
    let mut out = helpers::stamp_signed_pdf(&original, &stamp_fields, &verify_url, tz)
        .map_err(|e| anyhow::anyhow!(e))?;
    // The PAdES platform seal; on failure the flattened (unsealed) PDF is stored.
    let sealed = match granary_services::pdf_sign::sign_pdf(&out).await {
        Ok(r) => Ok(r),
        Err(_) => match helpers::reserialize_pdf(&out) {
            Ok(out2) => granary_services::pdf_sign::sign_pdf(&out2).await,
            Err(e) => Err(anyhow::anyhow!(e)),
        },
    };
    match sealed {
        Ok((bytes, seal_level)) => {
            out = bytes;
            doc.sealed_at = Some(now_micro());
            doc.seal_fingerprint = granary_services::pdf_sign::cert_fingerprint();
            doc.seal_level = Some(seal_level);
            doc.signature_level = Some(if granary_services::pdf_sign::is_qualified() {
                "qes".to_string()
            } else {
                doc.signature_level
                    .clone()
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| "ses".into())
            });
            sqlx::query(
                "UPDATE sign_documents SET sealed_at = $2, seal_fingerprint = $3, seal_level = $4, \
                 signature_level = $5, updated_at = now() WHERE id = $1",
            )
            .bind(doc.id)
            .bind(doc.sealed_at)
            .bind(&doc.seal_fingerprint)
            .bind(&doc.seal_level)
            .bind(&doc.signature_level)
            .execute(&mut *conn)
            .await?;
        }
        Err(se) => {
            tracing::error!(error = %se, doc_id = %doc.id, "pdf_seal_failed");
        }
    }
    let url = helpers::routers_signing_upload_to_minio(
        out,
        format!("signed/{}.pdf", doc.id),
        "application/pdf".into(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(url)
}

async fn set_signed_url(
    conn: &mut PgConnection,
    doc: &mut SignDocument,
    url: String,
) -> ApiResult<()> {
    sqlx::query("UPDATE sign_documents SET signed_file_url = $2, updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .bind(&url)
        .execute(&mut *conn)
        .await?;
    doc.signed_file_url = Some(url);
    Ok(())
}

async fn set_certificate(
    conn: &mut PgConnection,
    doc: &mut SignDocument,
    url: String,
    at: DateTime<Utc>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE sign_documents SET certificate_url = $2, certificate_generated_at = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(doc.id)
    .bind(&url)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    doc.certificate_url = Some(url);
    doc.certificate_generated_at = Some(at);
    Ok(())
}

// ─── Internal document management ────────────────────────────────────────────

granary_core::py_model! {
    pub struct ListDocumentsQ {
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /documents`: the caller's documents (superadmin: all), newest first.
pub async fn list_documents(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ListDocumentsQ>,
) -> ApiResult<Response> {
    require_sign_view(&user)?;
    let mut conn = db.conn().await?;
    let mut sql =
        String::from("SELECT * FROM sign_documents WHERE sign_documents.is_deleted = false");
    let mut n = 0;
    if !user.is_superadmin {
        n += 1;
        sql += &format!(" AND sign_documents.created_by_id = ${n}");
    }
    let status = q.status.filter(|s| !s.is_empty());
    if status.is_some() {
        n += 1;
        sql += &format!(" AND sign_documents.status = ${n}");
    }
    sql += &format!(
        " ORDER BY sign_documents.created_at DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    );
    let mut query = sqlx::query_as::<_, SignDocument>(&sql);
    if !user.is_superadmin {
        query = query.bind(user.id);
    }
    if let Some(s) = &status {
        query = query.bind(s);
    }
    let docs = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let ids: Vec<Uuid> = docs.iter().map(|d| d.id).collect();
    let signers: Vec<SignSigner> = if ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as(
            "SELECT * FROM sign_signers WHERE sign_signers.document_id = ANY($1) ORDER BY sign_signers.sign_order",
        )
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?
    };
    let out: Vec<Value> = docs
        .iter()
        .map(|d| {
            let mine: Vec<SignSigner> = signers
                .iter()
                .filter(|s| s.document_id == d.id)
                .cloned()
                .collect();
            doc_dict(d, &mine, None, None)
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /documents` (201): a new signing request, optionally with its PDF.
pub async fn create_document(
    InternalUser(user): InternalUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = granary_core::validation::Cx::new();
    let title = form.req_text(&mut cx, "title");
    let message = form.opt_text("message");
    // `source_type or None`, `source_id or None`.
    let source_type = form.opt_text("source_type").filter(|s| !s.is_empty());
    let source_id = form.opt_text("source_id").filter(|s| !s.is_empty());
    let file = form.opt_file(&mut cx, "file");
    form.check(cx)?;
    let title = title.expect("validated");
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    // A source_id string the UUID column refuses is asyncpg's DataError.
    let source_uuid = match &source_id {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let doc_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sign_documents (id, title, status, source_type, source_id, signature_level, jurisdictions, \
         message, created_by_id, is_deleted) VALUES ($1, $2, 'draft', $3, $4, 'ses', 'US,EU,ID', $5, $6, false)",
    )
    .bind(doc_id)
    .bind(&title)
    .bind(&source_type)
    .bind(source_uuid)
    .bind(&message)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    if let Some(file) = file {
        let content = file.data.to_vec();
        let hash = granary_compat::crypto::hashes::sha256_hex(&content);
        let url = helpers::routers_signing_upload_to_minio(
            content,
            format!("originals/{doc_id}.pdf"),
            "application/pdf".into(),
        )
        .await?;
        sqlx::query(
            "UPDATE sign_documents SET document_hash = $2, original_file_url = $3, updated_at = now() WHERE id = $1",
        )
        .bind(doc_id)
        .bind(hash)
        .bind(url)
        .execute(&mut *conn)
        .await?;
    }
    add_audit(
        &mut conn,
        doc_id,
        "document_created",
        Some(&format!("Document '{title}' created by {}", user.email)),
        None,
        None,
        None,
    )
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let doc: SignDocument = sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
        .bind(doc_id)
        .fetch_one(&mut *conn)
        .await?;
    let l = load_full(&mut conn, doc).await?;
    Ok(resp::json_status(
        201,
        doc_dict(&l.doc, &l.signers, Some((&l.fields, &l.logs)), Some(&user)),
    ))
}

/// `POST /documents/{doc_id}/upload-pdf`: upload or replace a draft's PDF.
pub async fn upload_pdf(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = granary_core::validation::Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.expect("validated");
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            "Can only upload PDF on draft documents",
        ));
    }
    let content = file.data.to_vec();
    let hash = granary_compat::crypto::hashes::sha256_hex(&content);
    let url = helpers::routers_signing_upload_to_minio(
        content,
        format!("originals/{doc_id}.pdf"),
        "application/pdf".into(),
    )
    .await?;
    sqlx::query(
        "UPDATE sign_documents SET document_hash = $2, original_file_url = $3, updated_at = now() WHERE id = $1",
    )
    .bind(doc.id)
    .bind(&hash)
    .bind(url)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "document_hash": hash,
        "file_url": format!("/api/v1/sign/documents/{doc_id}/pdf"),
    })))
}

/// `GET /documents/{doc_id}`: the sender's own document (superadmin: any) with signers, fields and trail.
pub async fn get_document(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_view(&user)?;
    let id = pg_uuid(&doc_id)?;
    let mut conn = db.conn().await?;
    let doc: Option<SignDocument> = sqlx::query_as(
        "SELECT * FROM sign_documents WHERE sign_documents.id = $1 AND sign_documents.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    // Same visibility as list_documents: the sender's own documents (superadmin sees all).
    let Some(doc) = doc.filter(|d| user.is_superadmin || d.created_by_id == Some(user.id)) else {
        return Err(ApiError::http(404, "Document not found"));
    };
    let l = load_full(&mut conn, doc).await?;
    Ok(resp::json(doc_dict(
        &l.doc,
        &l.signers,
        Some((&l.fields, &l.logs)),
        Some(&user),
    )))
}

/// `GET /documents/{doc_id}/pdf`: the original (or, once completed, the signed) PDF.
pub async fn get_document_pdf(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_view(&user)?;
    let id = parse_uuid(&doc_id, "doc_id")?;
    let mut conn = db.conn().await?;
    let doc: Option<SignDocument> =
        sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(doc) = doc else {
        return Err(ApiError::http(404, "Document not found"));
    };
    if !user.is_superadmin && doc.created_by_id != Some(user.id) {
        return Err(ApiError::http(403, "Access denied"));
    }
    let file_url = served_file(&doc);
    let Some(file_url) = file_url else {
        return Err(ApiError::http(404, "No PDF uploaded for this document"));
    };
    drop(conn);
    // Not caught in Python: a storage failure is the unhandled 500.
    let data = minio_bytes(&file_url).await.map_err(ApiError::internal)?;
    let safe = underscore_title(&doc.title);
    Ok(pdf_response(
        data,
        content_disposition("inline", &format!("{safe}.pdf")),
    ))
}

/// The signed PDF once completed, else the original (`file_url = ... if ... else ...`).
fn served_file(doc: &SignDocument) -> Option<String> {
    let signed = doc
        .signed_file_url
        .clone()
        .filter(|u| !u.is_empty() && doc.status.as_deref() == Some("completed"));
    signed
        .or_else(|| doc.original_file_url.clone())
        .filter(|u| !u.is_empty())
}

/// `GET /documents/{doc_id}/signed`: the flattened, sealed PDF (generated on demand for older documents).
pub async fn download_signed_pdf(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_view(&user)?;
    let mut conn = db.conn().await?;
    let mut doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() != Some("completed") {
        return Err(ApiError::http(400, "Document is not completed"));
    }
    if doc.signed_file_url.as_deref().is_none_or(|u| u.is_empty()) {
        if let Some(url) = generate_signed_pdf(&mut conn, &mut doc).await {
            set_signed_url(&mut conn, &mut doc, url).await?;
            drop(conn);
            db.commit().await?;
        } else {
            drop(conn);
        }
    } else {
        drop(conn);
    }
    let Some(url) = doc.signed_file_url.clone().filter(|u| !u.is_empty()) else {
        return Err(ApiError::http(404, "Signed PDF not available"));
    };
    let safe = safe_title(&doc.title);
    let data = minio_bytes(&url).await.map_err(ApiError::internal)?;
    Ok(pdf_response(
        data,
        content_disposition("attachment", &format!("{safe}_signed.pdf")),
    ))
}

/// `GET /documents/{doc_id}/certificate`: the signing certificate (generated on demand).
pub async fn download_certificate_internal(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_view(&user)?;
    let mut conn = db.conn().await?;
    let mut doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() != Some("completed") {
        return Err(ApiError::http(400, "Document is not completed"));
    }
    if doc.certificate_url.as_deref().is_none_or(|u| u.is_empty()) {
        let signers = signers_of(&mut conn, doc.id).await?;
        let logs = logs_of(&mut conn, doc.id).await?;
        if let Some(url) = generate_certificate(&doc, &signers, &logs).await {
            set_certificate(&mut conn, &mut doc, url, now_micro()).await?;
            drop(conn);
            db.commit().await?;
        } else {
            drop(conn);
        }
    } else {
        drop(conn);
    }
    let Some(url) = doc.certificate_url.clone().filter(|u| !u.is_empty()) else {
        return Err(ApiError::http(404, "Certificate not available"));
    };
    let safe = safe_title(&doc.title);
    let data = minio_bytes(&url).await.map_err(ApiError::internal)?;
    Ok(pdf_response(
        data,
        content_disposition("attachment", &format!("{safe}_certificate.pdf")),
    ))
}

/// `POST /documents/{doc_id}/finalize`: (re)generate the signed PDF and the certificate.
pub async fn finalize_document(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() != Some("completed") {
        return Err(ApiError::http(400, "Document is not completed"));
    }
    let result: ApiResult<()> = async {
        if let Some(signed) = generate_signed_pdf(&mut conn, &mut doc).await {
            set_signed_url(&mut conn, &mut doc, signed).await?;
        }
        let signers = signers_of(&mut conn, doc.id).await?;
        let logs = logs_of(&mut conn, doc.id).await?;
        if let Some(cert) = generate_certificate(&doc, &signers, &logs).await {
            set_certificate(&mut conn, &mut doc, cert, now_micro()).await?;
        }
        Ok(())
    }
    .await;
    drop(conn);
    let result = match result {
        Ok(()) => db.commit().await,
        Err(e) => Err(e),
    };
    if let Err(exc) = result {
        let _ = db.rollback().await;
        tracing::error!(error = %exc, doc_id = %doc_id, "finalize_failed");
        return Err(ApiError::http(502, format!("Finalize failed: {exc}")));
    }
    Ok(resp::json(json!({
        "signed": doc.signed_file_url.as_deref().is_some_and(|u| !u.is_empty()),
        "certificate": doc.certificate_url.as_deref().is_some_and(|u| !u.is_empty()),
    })))
}

granary_core::py_model! {
    pub struct ExportQ {
        pub status: String = "completed".to_string(),
    }
}

/// `GET /export`: a ZIP of every document in `status` (signed PDF or original, plus certificate).
pub async fn download_all(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ExportQ>,
) -> ApiResult<Response> {
    require_sign_view(&user)?;
    let mut conn = db.conn().await?;
    let mut sql = String::from(
        "SELECT * FROM sign_documents WHERE sign_documents.is_deleted = false AND sign_documents.status = $1",
    );
    if !user.is_superadmin {
        sql += " AND sign_documents.created_by_id = $2";
    }
    sql += " ORDER BY sign_documents.completed_at DESC NULLS LAST";
    let mut query = sqlx::query_as::<_, SignDocument>(&sql).bind(&q.status);
    if !user.is_superadmin {
        query = query.bind(user.id);
    }
    let docs = query.fetch_all(&mut *conn).await?;
    if docs.is_empty() {
        return Err(ApiError::http(404, "No documents to export"));
    }
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for mut doc in docs {
        let safe = safe_title(&doc.title);
        if doc.signed_file_url.as_deref().is_none_or(|u| u.is_empty())
            && doc.status.as_deref() == Some("completed")
        {
            if let Some(url) = generate_signed_pdf(&mut conn, &mut doc).await {
                set_signed_url(&mut conn, &mut doc, url).await?;
            }
        }
        if let Some(url) = doc.signed_file_url.clone().filter(|u| !u.is_empty()) {
            if let Ok(b) = minio_bytes(&url).await {
                entries.push((format!("{safe}/{safe}_signed.pdf"), b));
            }
        } else if let Some(url) = doc.original_file_url.clone().filter(|u| !u.is_empty()) {
            if let Ok(b) = minio_bytes(&url).await {
                entries.push((format!("{safe}/{safe}.pdf"), b));
            }
        }
        if let Some(url) = doc.certificate_url.clone().filter(|u| !u.is_empty()) {
            if let Ok(b) = minio_bytes(&url).await {
                entries.push((format!("{safe}/{safe}_certificate.pdf"), b));
            }
        }
    }
    drop(conn);
    db.commit().await?;
    let zip = helpers::zip_deflated(&entries, chrono::Local::now());
    Ok(resp::with_headers(
        resp::bytes_response(200, zip, "application/zip"),
        &[(
            "content-disposition",
            "attachment; filename=\"signed_documents.zip\"".to_string(),
        )],
    ))
}

/// `DELETE /documents/{doc_id}` (204): cancels (soft-deletes) a document that is not completed.
pub async fn cancel_document(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() == Some("completed") {
        return Err(ApiError::http(400, "Cannot cancel a completed document"));
    }
    sqlx::query(
        "UPDATE sign_documents SET status = 'cancelled', is_deleted = true, deleted_at = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(doc.id)
    .bind(now_micro())
    .execute(&mut *conn)
    .await?;
    add_audit(
        &mut conn,
        doc.id,
        "document_cancelled",
        Some(&format!("Cancelled by {}", user.email)),
        None,
        None,
        None,
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Signers & fields ─────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct SearchQ {
        pub r#type: String = "internal".to_string(),
        pub q: String = String::new(),
        pub limit: i64 = 20,
    }
}

/// `GET /signers/search`: internal users, partner accounts or vendors by name/email.
pub async fn search_signers(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(params): PyQuery<SearchQ>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let q = py_strip(&params.q).to_string();
    let like = format!("%{q}%");
    let mut conn = db.conn().await?;
    let out: Vec<Value> = match params.r#type.as_str() {
        "internal" => {
            let mut sql = String::from(
                "SELECT id, name, email, position, department FROM users WHERE true AND users.is_active = true \
                 AND users.portal_type = 'internal'",
            );
            if !q.is_empty() {
                sql += " AND (users.name ILIKE $2 OR users.email ILIKE $2)";
            }
            sql += " ORDER BY users.name LIMIT $1";
            let mut query =
                sqlx::query_as::<_, (Uuid, String, String, Option<String>, Option<String>)>(&sql)
                    .bind(params.limit);
            if !q.is_empty() {
                query = query.bind(&like);
            }
            query
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|(id, name, email, position, department)| {
                    let subtitle = position
                        .filter(|p| !p.is_empty())
                        .or(department.filter(|d| !d.is_empty()))
                        .unwrap_or_default();
                    json!({"id": id.to_string(), "name": name, "email": email, "subtitle": subtitle,
                           "signer_type": "internal"})
                })
                .collect()
        }
        "partner" => {
            let mut sql = String::from(
                "SELECT id, company_name, contact_name, contact_email FROM partner_accounts \
                 WHERE partner_accounts.is_deleted = false AND partner_accounts.status = 'active'",
            );
            if !q.is_empty() {
                sql += " AND (partner_accounts.company_name ILIKE $2 OR partner_accounts.contact_name ILIKE $2 \
                        OR partner_accounts.contact_email ILIKE $2)";
            }
            sql += " ORDER BY partner_accounts.company_name LIMIT $1";
            let mut query =
                sqlx::query_as::<_, (Uuid, Option<String>, Option<String>, Option<String>)>(&sql)
                    .bind(params.limit);
            if !q.is_empty() {
                query = query.bind(&like);
            }
            query
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|(id, company, contact, email)| {
                    json!({"id": id.to_string(),
                           "name": contact.filter(|c| !c.is_empty()).or_else(|| company.clone()),
                           "email": email.filter(|e| !e.is_empty()).unwrap_or_default(),
                           "subtitle": company, "signer_type": "partner",
                           "partner_account_id": id.to_string()})
                })
                .collect()
        }
        "vendor" => {
            let mut sql = String::from(
                "SELECT id, name, contact_name, contact_email FROM vendors WHERE vendors.is_deleted = false \
                 AND vendors.status = 'active'",
            );
            if !q.is_empty() {
                sql += " AND (vendors.name ILIKE $2 OR vendors.contact_name ILIKE $2 OR vendors.contact_email ILIKE $2)";
            }
            sql += " ORDER BY vendors.name LIMIT $1";
            let mut query =
                sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>)>(&sql)
                    .bind(params.limit);
            if !q.is_empty() {
                query = query.bind(&like);
            }
            query
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|(id, name, contact, email)| {
                    json!({"id": id.to_string(),
                           "name": contact.filter(|c| !c.is_empty()).unwrap_or_else(|| name.clone()),
                           "email": email.filter(|e| !e.is_empty()).unwrap_or_default(),
                           "subtitle": name, "signer_type": "vendor"})
                })
                .collect()
        }
        _ => vec![],
    };
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class SignerAdd(BaseModel)`.
    pub struct SignerAdd {
        pub name: String,
        pub email: String,
        pub sign_order: i64 = 1,
        pub role: String = "signer".to_string(),
        pub user_id: Option<String> = None,
        pub signer_type: String = "external".to_string(),
        pub partner_account_id: Option<String> = None,
    }
}

fn opt_pg_uuid(v: &Option<String>) -> ApiResult<Option<Uuid>> {
    match v.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Ok(Some(pg_uuid(s)?)),
        None => Ok(None),
    }
}

/// `POST /documents/{doc_id}/signers` (201).
pub async fn add_signer(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<SignerAdd>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            "Can only add signers to draft documents",
        ));
    }
    let user_id = opt_pg_uuid(&body.user_id)?;
    let partner_id = opt_pg_uuid(&body.partner_account_id)?;
    let sign_order = i32::try_from(body.sign_order)
        .map_err(|_| ApiError::internal(anyhow::anyhow!("DataError: value out of int32 range")))?;
    // A signer linked to a user gets that user's name and email: the signing page offers the user's saved
    // signature, so the link must never point at someone else's address.
    let (name, email) = match user_id {
        Some(uid) => {
            let found: Option<(String, String)> =
                sqlx::query_as("SELECT name, email FROM users WHERE id = $1")
                    .bind(uid)
                    .fetch_optional(&mut *conn)
                    .await?;
            found.ok_or_else(|| ApiError::http(400, "That user does not exist."))?
        }
        None => (body.name.clone(), body.email.clone()),
    };
    let signer: SignSigner = sqlx::query_as(
        "INSERT INTO sign_signers (id, document_id, name, email, sign_order, role, token, status, user_id, \
         signer_type, partner_account_id) VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', $8, $9, $10) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(doc.id)
    .bind(&name)
    .bind(email.to_lowercase())
    .bind(sign_order)
    .bind(&body.role)
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&body.signer_type)
    .bind(partner_id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, signer_dict(&signer, None)))
}

/// `DELETE /documents/{doc_id}/signers/{signer_id}` (204).
pub async fn remove_signer(
    InternalUser(user): InternalUser,
    db: Db,
    Path((doc_id, signer_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            "Can only remove signers from draft documents",
        ));
    }
    let sid = parse_uuid(&signer_id, "signer_id")?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sign_signers WHERE id = $1 AND document_id = $2")
            .bind(sid)
            .bind(doc.id)
            .fetch_optional(&mut *conn)
            .await?;
    if found.is_none() {
        return Err(ApiError::http(404, "Signer not found"));
    }
    sqlx::query("DELETE FROM sign_signers WHERE id = $1")
        .bind(sid)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `POST /documents/{doc_id}/self-sign` (201): the caller as a signer, with their signing token.
pub async fn self_sign(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    let signers = signers_of(&mut conn, doc.id).await?;
    if doc.status.as_deref() != Some("draft") {
        return Err(ApiError::http(400, "Can only self-sign draft documents"));
    }
    let email_lower = user.email.to_lowercase();
    if let Some(existing) = signers.iter().find(|s| s.email == email_lower) {
        return Ok(resp::json_status(
            201,
            json!({"token": existing.token.to_string(), "signer": signer_dict(existing, Some(&user))}),
        ));
    }
    let max_order = signers.iter().map(|s| s.sign_order).max().unwrap_or(0);
    let signer: SignSigner = sqlx::query_as(
        "INSERT INTO sign_signers (id, document_id, name, email, sign_order, role, token, status, user_id, \
         signer_type) VALUES ($1, $2, $3, $4, $5, 'signer', $6, 'pending', $7, 'internal') RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(doc.id)
    .bind(&user.name)
    .bind(&email_lower)
    .bind(max_order + 1)
    .bind(Uuid::new_v4())
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"token": signer.token.to_string(), "signer": signer_dict(&signer, Some(&user))}),
    ))
}

granary_core::py_model! {
    /// `class FieldAdd(BaseModel)`.
    pub struct FieldAdd {
        pub signer_id: Option<String> = None,
        pub field_type: String,
        pub page_number: i64 = 1,
        pub x: Option<f64> = None,
        pub y: Option<f64> = None,
        pub width: Option<f64> = None,
        pub height: Option<f64> = None,
        pub is_required: bool = true,
        pub label: Option<String> = None,
        pub value: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class FieldBatchPayload(BaseModel)`.
    pub struct FieldBatchPayload {
        pub fields: Vec<FieldAdd>,
    }
}

/// A float bound to a NUMERIC column the way asyncpg encodes it (`Decimal(float)`, exact).
fn float_dec(v: Option<f64>) -> Option<Decimal> {
    v.and_then(Decimal::from_f64_retain)
}

async fn insert_field(conn: &mut PgConnection, doc_id: Uuid, f: &FieldAdd) -> ApiResult<SignField> {
    let page = i32::try_from(f.page_number)
        .map_err(|_| ApiError::internal(anyhow::anyhow!("DataError: value out of int32 range")))?;
    Ok(sqlx::query_as(
        "INSERT INTO sign_fields (id, document_id, signer_id, field_type, page_number, x, y, width, height, \
         is_required, label, value) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(doc_id)
    .bind(opt_pg_uuid(&f.signer_id)?)
    .bind(&f.field_type)
    .bind(page)
    .bind(float_dec(f.x))
    .bind(float_dec(f.y))
    .bind(float_dec(f.width))
    .bind(float_dec(f.height))
    .bind(f.is_required)
    .bind(&f.label)
    .bind(&f.value)
    .fetch_one(&mut *conn)
    .await?)
}

/// `POST /documents/{doc_id}/fields` (201).
pub async fn add_field(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<FieldAdd>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    let field = insert_field(&mut conn, doc.id, &body).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, field_dict(&field)))
}

/// `POST /documents/{doc_id}/fields/batch` (201): replaces every field of a draft.
pub async fn add_fields_batch(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<FieldBatchPayload>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    if doc.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            "Can only edit the fields of draft documents",
        ));
    }
    sqlx::query("DELETE FROM sign_fields WHERE sign_fields.document_id = $1")
        .bind(doc.id)
        .execute(&mut *conn)
        .await?;
    let mut created = Vec::with_capacity(body.fields.len());
    for f in &body.fields {
        created.push(insert_field(&mut conn, doc.id, f).await?);
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        Value::Array(created.iter().map(field_dict).collect()),
    ))
}

// ─── Send invitations ─────────────────────────────────────────────────────────

fn spawn_invitations(invites: Vec<(String, String)>) {
    for (doc_id, signer_id) in invites {
        crate::lifecycle::spawn_scoped(async move {
            let _ = helpers::routers_signing_send_signing_invitation(doc_id, signer_id).await;
        });
    }
}

/// `POST /documents/{doc_id}/send`: draft -> pending, inviting the first signing step.
pub async fn send_document(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    let signers = signers_of(&mut conn, doc.id).await?;
    if doc.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "Document is already {}",
                doc.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    if doc
        .original_file_url
        .as_deref()
        .is_none_or(|u| u.is_empty())
    {
        return Err(ApiError::http(400, "Upload a PDF before sending"));
    }
    if signers.is_empty() {
        return Err(ApiError::http(
            400,
            "Add at least one signer before sending",
        ));
    }
    let now = now_micro();
    let orders: Vec<i32> = signers
        .iter()
        .filter(|s| s.role.as_deref() == Some("signer"))
        .map(|s| s.sign_order)
        .collect();
    let Some(min_order) = orders.iter().min().copied() else {
        return Err(ApiError::http(
            400,
            "Add at least one signer (not only CC recipients) before sending",
        ));
    };
    sqlx::query("UPDATE sign_documents SET status = 'pending', updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *conn)
        .await?;
    add_audit(
        &mut conn,
        doc.id,
        "document_sent",
        Some(&format!("Sent for signing by {}", user.email)),
        None,
        None,
        None,
    )
    .await?;
    let mut invites = Vec::new();
    for s in &signers {
        if s.sign_order == min_order && s.role.as_deref() == Some("signer") {
            sqlx::query(
                "UPDATE sign_signers SET status = 'sent', sent_at = $2, updated_at = now() WHERE id = $1",
            )
            .bind(s.id)
            .bind(now)
            .execute(&mut *conn)
            .await?;
            add_audit(
                &mut conn,
                doc.id,
                "sent_to_signer",
                Some(&format!("Invitation sent to {}", s.email)),
                Some(s.id),
                None,
                None,
            )
            .await?;
            invites.push((doc.id.to_string(), s.id.to_string()));
        }
    }
    let sent_count = invites.len();
    drop(conn);
    db.commit().await?;
    spawn_invitations(invites);
    Ok(resp::json(
        json!({"status": "pending", "invitations_sent": sent_count}),
    ))
}

granary_core::py_model! {
    pub struct RemindQ {
        pub signer_id: String,
    }
}

/// `POST /documents/{doc_id}/remind?signer_id=`: re-sends a pending signer's invitation.
pub async fn remind_signer(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyQuery(q): PyQuery<RemindQ>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut conn = db.conn().await?;
    let doc = load_owned_doc(&mut conn, &doc_id, &user).await?;
    let sid = parse_uuid(&q.signer_id, "signer_id")?;
    let signer: Option<SignSigner> = sqlx::query_as(
        "SELECT * FROM sign_signers WHERE id = $1 AND document_id = $2 AND status IN ('pending', 'sent', 'viewed')",
    )
    .bind(sid)
    .bind(doc.id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(signer) = signer else {
        return Err(ApiError::http(404, "Pending signer not found"));
    };
    helpers::routers_signing_send_signing_invitation(doc.id.to_string(), signer.id.to_string())
        .await?;
    add_audit(
        &mut conn,
        doc.id,
        "reminder_sent",
        Some(&format!("Reminder sent to {}", signer.email)),
        Some(signer.id),
        None,
        None,
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"sent": true})))
}

// ─── Public signing endpoints (no auth) ──────────────────────────────────────

/// The signer behind a token and its document (`selectinload(SignSigner.document)`).
async fn signer_by_token(
    conn: &mut PgConnection,
    token: &str,
) -> ApiResult<Option<(SignSigner, Option<SignDocument>)>> {
    let t = pg_uuid(token)?;
    let signer: Option<SignSigner> =
        sqlx::query_as("SELECT * FROM sign_signers WHERE sign_signers.token = $1")
            .bind(t)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(signer) = signer else {
        return Ok(None);
    };
    let doc: Option<SignDocument> = sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
        .bind(signer.document_id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(Some((signer, doc)))
}

fn expired(doc: &SignDocument) -> bool {
    doc.expires_at.is_some_and(|e| Utc::now() > e)
}

/// The checks every public signing-link route makes: the link exists, its document is not deleted, is out for
/// signing or completed (not a draft, cancelled or declined one), and has not expired.
async fn open_public_link(
    conn: &mut PgConnection,
    token: &str,
) -> ApiResult<(SignSigner, SignDocument)> {
    let Some((signer, doc)) = signer_by_token(conn, token).await? else {
        return Err(ApiError::http(404, "Signing link not found or expired"));
    };
    let Some(doc) = doc
        .filter(|d| !d.is_deleted && matches!(d.status.as_deref(), Some("pending" | "completed")))
    else {
        return Err(ApiError::http(
            400,
            "This document is not available for signing",
        ));
    };
    if expired(&doc) {
        return Err(ApiError::http(410, "This signing link has expired"));
    }
    Ok((signer, doc))
}

/// `GET /public/{token}`: what the signer sees before signing.
pub async fn public_view_document(db: Db, Path(token): Path<String>) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let (signer, doc) = open_public_link(&mut conn, &token).await?;
    let fields: Vec<SignField> = sqlx::query_as(
        "SELECT * FROM sign_fields WHERE sign_fields.document_id = $1 AND (sign_fields.signer_id = $2 \
         OR sign_fields.signer_id IS NULL)",
    )
    .bind(doc.id)
    .bind(signer.id)
    .fetch_all(&mut *conn)
    .await?;
    // The user's saved signature only when this signer IS that user (same email): a signer row linked to
    // someone else's account must not hand their signature image to whoever holds the link.
    let mut saved_signature: Option<String> = None;
    if let Some(uid) = signer.user_id {
        let sig: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT email, signature_data FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some((email, Some(s))) = sig {
            if !s.is_empty() && email.to_lowercase() == signer.email.to_lowercase() {
                saved_signature = Some(s);
            }
        }
    }
    let level = doc
        .signature_level
        .as_deref()
        .filter(|l| !l.is_empty())
        .unwrap_or("ses");
    Ok(resp::json(json!({
        "document_id": doc.id.to_string(),
        "document_title": doc.title,
        "document_message": doc.message,
        "document_status": doc.status,
        "signer_id": signer.id.to_string(),
        "signer_name": signer.name,
        "signer_email": signer.email,
        "signer_status": signer.status,
        "signer_role": signer.role,
        "pdf_url": format!("/api/v1/sign/public/{token}/pdf"),
        "fields": fields.iter().map(field_dict).collect::<Vec<_>>(),
        "already_signed": signer.status.as_deref() == Some("signed"),
        "already_declined": signer.status.as_deref() == Some("declined"),
        "saved_signature": saved_signature,
        "disclosure_text": ESIGN_DISCLOSURE,
        "disclosure_version": ESIGN_DISCLOSURE_VERSION,
        "signature_level": level,
        "signature_level_label": certificate::signature_level_label(level),
    })))
}

/// `GET /public/{token}/pdf`: the PDF for the signer (signed copy once completed).
pub async fn proxy_pdf(db: Db, Path(token): Path<String>) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let (_, doc) = open_public_link(&mut conn, &token).await?;
    drop(conn);
    let Some(file_url) = served_file(&doc) else {
        return Err(ApiError::http(404, "PDF not yet uploaded"));
    };
    match minio_bytes(&file_url).await {
        Ok(data) => Ok(pdf_response(
            data,
            content_disposition("inline", &format!("{}.pdf", doc.title)),
        )),
        Err(_) => Err(ApiError::http(404, "PDF not available")),
    }
}

/// `GET /public/{token}/certificate`: the certificate of a completed document.
pub async fn public_download_certificate(db: Db, Path(token): Path<String>) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let (_, doc) = open_public_link(&mut conn, &token).await?;
    drop(conn);
    if doc.status.as_deref() != Some("completed") {
        return Err(ApiError::http(400, "Document is not yet completed"));
    }
    let Some(url) = doc.certificate_url.clone().filter(|u| !u.is_empty()) else {
        return Err(ApiError::http(404, "Certificate not yet generated"));
    };
    match minio_bytes(&url).await {
        Ok(data) => {
            let safe = underscore_title(&doc.title);
            Ok(pdf_response(
                data,
                content_disposition("attachment", &format!("{safe}_certificate.pdf")),
            ))
        }
        Err(_) => Err(ApiError::http(404, "Certificate not available")),
    }
}

granary_core::py_model! {
    /// `class SignSubmit(BaseModel)`.
    pub struct SignSubmit {
        pub signature_data: Option<String> = None,
        pub initials_data: Option<String> = None,
        pub sign_reason: Option<String> = None,
        pub field_values: Vec<serde_json::Map<String, Value>> = Vec::new(),
        pub consent: bool = true,
    }
}

/// `base64.b64decode(s, validate=True)`.
fn b64decode_strict(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() {
        return None;
    }
    let body = s.trim_end_matches('=');
    let pads = s.len() - body.len();
    if pads > 2
        || !body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    {
        return None;
    }
    let n = body.len();
    let ok = match pads {
        0 => n.is_multiple_of(4),
        1 => n % 4 == 3,
        _ => n % 4 == 2,
    };
    if !ok {
        return None;
    }
    granary_compat::crypto::pyb64::b64decode_str(s).ok()
}

/// `_check_signature_image(value, field, max_bytes)`.
fn check_signature_image(value: Option<&str>, field: &str, max_bytes: i64) -> ApiResult<()> {
    let Some(value) = value else { return Ok(()) };
    if !value.starts_with(SIGNATURE_DATA_PREFIX) {
        return Err(ApiError::http(
            422,
            format!("{field} must be a PNG image (data:image/png;base64,\u{2026})"),
        ));
    }
    let Some(decoded) = b64decode_strict(&value[SIGNATURE_DATA_PREFIX.len()..]) else {
        return Err(ApiError::http(422, format!("{field} is not valid base64")));
    };
    if decoded.is_empty() || decoded.len() as i64 > max_bytes {
        return Err(ApiError::http(
            422,
            format!("{field} must be a non-empty image of at most {max_bytes} bytes"),
        ));
    }
    Ok(())
}

/// `POST /public/{token}/sign`: records the signature, invites the next step or completes the document.
pub async fn submit_signature(
    db: Db,
    Path(token): Path<String>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    PyJson(body): PyJson<SignSubmit>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let t = pg_uuid(&token)?;
    let found: Option<SignSigner> = sqlx::query_as("SELECT * FROM sign_signers WHERE token = $1")
        .bind(t)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(found) = found else {
        return Err(ApiError::http(404, "Signing link not found"));
    };
    // Lock the document: concurrent signers are serialized, so the last one completes it.
    let doc: SignDocument =
        sqlx::query_as("SELECT * FROM sign_documents WHERE sign_documents.id = $1 FOR UPDATE")
            .bind(found.document_id)
            .fetch_one(&mut *conn)
            .await?;
    let mut signers = signers_of(&mut conn, doc.id).await?;
    let mut fields = fields_of(&mut conn, doc.id).await?;
    let _ = logs_of(&mut conn, doc.id).await?;
    let idx = signers
        .iter()
        .position(|s| s.id == found.id)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("StopIteration")))?;
    if doc.status.as_deref() != Some("pending") {
        return Err(ApiError::http(
            400,
            format!(
                "Document is {}{EM} cannot sign",
                doc.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    if expired(&doc) {
        return Err(ApiError::http(410, "This signing link has expired"));
    }
    let signer = &signers[idx];
    match signer.status.as_deref() {
        Some("signed") => return Err(ApiError::http(400, "You have already signed this document")),
        Some("declined") => return Err(ApiError::http(400, "You have declined this document")),
        _ => {}
    }
    if signer.role.as_deref() != Some("signer") || signer.status.as_deref() == Some("pending") {
        return Err(ApiError::http(
            400,
            "This document is not waiting for your signature yet",
        ));
    }
    if !body.consent {
        return Err(ApiError::http(
            400,
            "You must consent to using electronic records and signatures to sign.",
        ));
    }
    if body.signature_data.is_some() || body.initials_data.is_some() {
        let max_bytes = granary_core::settings_store::require_int(
            &mut conn,
            SIGNATURE_MAX_BYTES_KEY,
            "Approvals & signing \u{2192} E-signing",
        )
        .await?;
        check_signature_image(body.signature_data.as_deref(), "signature_data", max_bytes)?;
        check_signature_image(body.initials_data.as_deref(), "initials_data", max_bytes)?;
    }

    let now = now_micro();
    let ua = user_agent(&headers);
    let ip = ip.as_deref();
    let signer_id = signers[idx].id;
    let signer_name = signers[idx].name.clone();
    let signer_email = signers[idx].email.clone();
    {
        let s = &mut signers[idx];
        if s.viewed_at.is_none() {
            s.viewed_at = Some(now);
        }
        s.consent_version = Some(ESIGN_DISCLOSURE_VERSION.into());
        s.consent_agreed_at = Some(now);
        s.consent_text_sha256 = Some(granary_compat::crypto::hashes::sha256_hex(
            ESIGN_DISCLOSURE.as_bytes(),
        ));
        s.status = Some("signed".into());
        s.signed_at = Some(now);
        s.ip_address = ip.map(str::to_string);
        s.user_agent = Some(ua.clone());
        if let Some(d) = body.signature_data.as_deref().filter(|d| !d.is_empty()) {
            s.signature_data = Some(d.to_string());
        }
        if let Some(d) = body.initials_data.as_deref().filter(|d| !d.is_empty()) {
            s.initials_data = Some(d.to_string());
        }
        if let Some(r) = body
            .sign_reason
            .as_deref()
            .filter(|r| SIGN_REASONS.contains(r))
        {
            s.sign_reason = Some(r.to_string());
        }
    }
    add_audit(
        &mut conn,
        doc.id,
        "consent_given",
        Some(&format!(
            "{signer_name} consented to electronic records & signatures (disclosure {ESIGN_DISCLOSURE_VERSION})"
        )),
        Some(signer_id),
        ip,
        Some(&ua),
    )
    .await?;
    add_audit(
        &mut conn,
        doc.id,
        "intent_affirmed",
        Some(&format!(
            "{signer_name} affirmed intent to sign and authority to do so"
        )),
        Some(signer_id),
        ip,
        Some(&ua),
    )
    .await?;
    {
        let s = &signers[idx];
        sqlx::query(
            "UPDATE sign_signers SET viewed_at = $2, consent_version = $3, consent_agreed_at = $4, \
             consent_text_sha256 = $5, status = $6, signed_at = $7, ip_address = $8, user_agent = $9, \
             signature_data = $10, initials_data = $11, sign_reason = $12, updated_at = now() WHERE id = $1",
        )
        .bind(s.id)
        .bind(s.viewed_at)
        .bind(&s.consent_version)
        .bind(s.consent_agreed_at)
        .bind(&s.consent_text_sha256)
        .bind(&s.status)
        .bind(s.signed_at)
        .bind(&s.ip_address)
        .bind(&s.user_agent)
        .bind(&s.signature_data)
        .bind(&s.initials_data)
        .bind(&s.sign_reason)
        .execute(&mut *conn)
        .await?;
    }
    // Field values: the signer's own fields, plus unassigned ones nobody has filled yet.
    for fv in &body.field_values {
        let fid = fv.get("field_id");
        if !fid.is_some_and(granary_services::whatsapp::py_truthy) {
            continue;
        }
        let fid = granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(
            fid.expect("checked"),
        ));
        let Some(field) = fields.iter_mut().find(|f| f.id.to_string() == fid) else {
            continue;
        };
        let own = field.signer_id == Some(signer_id);
        let open = field.signer_id.is_none() && field.value.as_deref().is_none_or(|v| v.is_empty());
        if own || open {
            let value = match fv.get("value") {
                Some(v) => {
                    granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(v))
                }
                None => String::new(),
            };
            sqlx::query("UPDATE sign_fields SET value = $2, updated_at = now() WHERE id = $1")
                .bind(field.id)
                .bind(&value)
                .execute(&mut *conn)
                .await?;
            field.value = Some(value);
        }
    }
    add_audit(
        &mut conn,
        doc.id,
        "document_signed",
        Some(&format!("{signer_name} <{signer_email}> signed")),
        Some(signer_id),
        ip,
        Some(&ua),
    )
    .await?;

    // Sequential signing: one step signs in parallel; the next step is invited once it is complete.
    let my_order = signers[idx].sign_order;
    let doc_signers: Vec<usize> = (0..signers.len())
        .filter(|i| signers[*i].role.as_deref() == Some("signer"))
        .collect();
    let step_open = doc_signers.iter().any(|i| {
        signers[*i].sign_order == my_order && signers[*i].status.as_deref() != Some("signed")
    });
    let mut waiting: Vec<usize> = doc_signers
        .iter()
        .copied()
        .filter(|i| signers[*i].status.as_deref() == Some("pending"))
        .collect();
    waiting.sort_by_key(|i| signers[*i].sign_order);
    let mut invites = Vec::new();
    let mut completed = false;
    if step_open {
        // others at this step still have to sign
    } else if let Some(first) = waiting.first() {
        let next_order = signers[*first].sign_order;
        for i in waiting.clone() {
            if signers[i].sign_order != next_order {
                continue;
            }
            signers[i].status = Some("sent".into());
            signers[i].sent_at = Some(now);
            sqlx::query(
                "UPDATE sign_signers SET status = 'sent', sent_at = $2, updated_at = now() WHERE id = $1",
            )
            .bind(signers[i].id)
            .bind(now)
            .execute(&mut *conn)
            .await?;
            add_audit(
                &mut conn,
                doc.id,
                "sent_to_signer",
                Some(&format!("Invitation sent to {}", signers[i].email)),
                Some(signers[i].id),
                None,
                None,
            )
            .await?;
            invites.push((doc.id.to_string(), signers[i].id.to_string()));
        }
    } else if doc_signers
        .iter()
        .all(|i| signers[*i].status.as_deref() == Some("signed"))
    {
        completed = true;
        sqlx::query(
            "UPDATE sign_documents SET status = 'completed', completed_at = $2, updated_at = now() WHERE id = $1",
        )
        .bind(doc.id)
        .bind(now)
        .execute(&mut *conn)
        .await?;
        add_audit(
            &mut conn,
            doc.id,
            "document_completed",
            Some("All signers have signed. Generating certificate."),
            None,
            None,
            None,
        )
        .await?;
        // Reload with every relationship (the final signer's events included) for the certificate.
        let mut doc_full: SignDocument =
            sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
                .bind(doc.id)
                .fetch_one(&mut *conn)
                .await?;
        if let Some(url) = generate_signed_pdf(&mut conn, &mut doc_full).await {
            set_signed_url(&mut conn, &mut doc_full, url).await?;
        }
        let full_signers = signers_of(&mut conn, doc.id).await?;
        let full_logs = logs_of(&mut conn, doc.id).await?;
        if let Some(cert) = generate_certificate(&doc_full, &full_signers, &full_logs).await {
            set_certificate(&mut conn, &mut doc_full, cert, now).await?;
            add_audit(
                &mut conn,
                doc.id,
                "certificate_generated",
                Some("Signing certificate PDF generated and stored"),
                None,
                None,
                None,
            )
            .await?;
        }
        notify_completion(&mut conn, &doc_full).await;
        granary_services::verification::signing_flow::on_sign_completed(
            &mut conn,
            &doc_full,
            &full_signers,
        )
        .await?;
    }
    drop(conn);
    db.commit().await?;
    spawn_invitations(invites);
    Ok(resp::json(json!({
        "status": "signed",
        "signed_at": iso(&now),
        "document_completed": completed,
    })))
}

/// `_notify_completion(doc, db)`: emails the creator; every error is swallowed.
async fn notify_completion(conn: &mut PgConnection, doc: &SignDocument) {
    let Some(creator_id) = doc.created_by_id else {
        return;
    };
    let _ = async {
        let creator: Option<(String, String)> =
            sqlx::query_as("SELECT email, name FROM users WHERE id = $1")
                .bind(creator_id)
                .fetch_optional(&mut *conn)
                .await?;
        let Some((email, name)) = creator else {
            return Ok(());
        };
        let cfg = granary_services::email::load_system_smtp(&mut *conn).await?;
        let s = settings();
        let brand = s
            .APP_NAME
            .split_whitespace()
            .next()
            .ok_or_else(|| anyhow::anyhow!("list index out of range"))?;
        let cert_text = if doc.certificate_url.as_deref().is_some_and(|u| !u.is_empty()) {
            format!(
                "<p>Download the signing certificate: <a href=\"{}/sign/documents\">View in ERP</a></p>",
                s.APP_URL
            )
        } else {
            String::new()
        };
        let html = format!(
            "\n        <html><body style=\"font-family:Arial,sans-serif;color:#111;max-width:560px;margin:40px auto;padding:0 20px\">\n          <div style=\"border-bottom:3px solid #1f4e79;padding-bottom:16px;margin-bottom:24px\">\n            <span style=\"font-size:24px;font-weight:900;color:#1f4e79;letter-spacing:4px\">{brand}</span>\n          </div>\n          <h2 style=\"color:#16a34a\">\u{2713} All parties have signed</h2>\n          <p>Hello <strong>{}</strong>,</p>\n          <p>All required signers have completed signing <strong>{}</strong>.</p>\n          {cert_text}\n        </body></html>",
            granary_compat::pyfmt::html_escape(&name, true),
            granary_compat::pyfmt::html_escape(&doc.title, true),
        );
        granary_services::email::_send_via_config(
            &cfg,
            std::slice::from_ref(&email),
            &format!("Signing Complete: {}", doc.title),
            &html,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
        anyhow::Ok(())
    }
    .await;
}

granary_core::py_model! {
    pub struct DeclineQ {
        pub reason: Option<String> = None,
    }
}

/// `POST /public/{token}/decline`.
pub async fn decline_signing(
    db: Db,
    Path(token): Path<String>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    PyQuery(q): PyQuery<DeclineQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some((mut signer, doc)) = signer_by_token(&mut conn, &token).await? else {
        return Err(ApiError::http(404, "Signing link not found"));
    };
    if matches!(signer.status.as_deref(), Some("signed" | "declined")) {
        return Err(ApiError::http(
            400,
            format!("Already {}", signer.status.as_deref().unwrap_or("None")),
        ));
    }
    let mut doc =
        doc.ok_or_else(|| ApiError::internal(anyhow::anyhow!("signer without a document")))?;
    if doc.status.as_deref() != Some("pending") {
        return Err(ApiError::http(
            400,
            format!(
                "Document is {}{EM} cannot decline",
                doc.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    if expired(&doc) {
        return Err(ApiError::http(410, "This signing link has expired"));
    }
    let ua = user_agent(&headers);
    signer.status = Some("declined".into());
    signer.declined_reason = q.reason.clone();
    signer.ip_address = ip.clone();
    signer.user_agent = Some(ua.clone());
    doc.status = Some("declined".into());
    sqlx::query(
        "UPDATE sign_signers SET status = 'declined', declined_reason = $2, ip_address = $3, user_agent = $4, \
         updated_at = now() WHERE id = $1",
    )
    .bind(signer.id)
    .bind(&q.reason)
    .bind(&ip)
    .bind(&ua)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE sign_documents SET status = 'declined', updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *conn)
        .await?;
    granary_services::verification::signing_flow::on_sign_declined(&mut conn, &doc, &signer)
        .await?;
    add_audit(
        &mut conn,
        doc.id,
        "document_declined",
        Some(&format!(
            "{} declined: {}",
            signer.name,
            q.reason
                .as_deref()
                .filter(|r| !r.is_empty())
                .unwrap_or("No reason given")
        )),
        Some(signer.id),
        ip.as_deref(),
        Some(&ua),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "declined"})))
}

// ─── Public verification ──────────────────────────────────────────────────────

/// `GET /verify/{document_id}`: integrity evidence only (no titles or identities).
pub async fn verify_document(db: Db, Path(document_id): Path<String>) -> ApiResult<Response> {
    let id = pg_uuid(&document_id)?;
    let mut conn = db.conn().await?;
    let doc: Option<SignDocument> = sqlx::query_as(
        "SELECT * FROM sign_documents WHERE sign_documents.id = $1 AND sign_documents.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(doc) = doc else {
        return Err(ApiError::http(404, "Document not found"));
    };
    let sealed = doc.sealed_at.is_some();
    let level = doc
        .signature_level
        .as_deref()
        .filter(|l| !l.is_empty())
        .unwrap_or("ses");
    let seal = doc
        .seal_level
        .as_deref()
        .filter(|l| !l.is_empty())
        .unwrap_or("none");
    Ok(resp::json(json!({
        "document_id": doc.id.to_string(),
        "status": doc.status,
        "document_hash": doc.document_hash,
        "hash_algorithm": "SHA-256",
        "completed_at": iso_opt(doc.completed_at.as_ref()),
        "digitally_sealed": sealed,
        "seal_authority": if sealed { Value::String(settings().SIGN_SEAL_NAME.clone()) } else { Value::Null },
        "sealed_at": iso_opt(doc.sealed_at.as_ref()),
        "signature_level_label": certificate::signature_level_label(level),
        "seal_level_label": certificate::seal_level_label(seal),
        "notice": "Scan the QR code printed on the document to see its full verification details.",
    })))
}
