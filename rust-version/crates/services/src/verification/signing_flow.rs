//! Port of backend/app/services/verification/signing_flow.py.
//!
//! E-signature straight from an approved document.
//!
//! `start_from_document` issues the document's verification (which enforces the approval gate and renders
//! the PDF with its QR on every page), uploads that exact PDF as the signing original, places each
//! signer's signature field on the anchors the template prints, and invites the first signer:
//!
//! - counterparty:               our approval (stamped on the PDF, verifiable by QR) is our side; only the
//!   customer / candidate signs, within `sign_deadline_days` business days,
//!   extendable by at most `max_extension_days` business days;
//! - internal:                   the policy's signer role (the Owner) signs (legacy);
//! - internal_then_counterparty: the Owner signs first, then the counterparty (legacy).
//!
//! When every party has signed, the outcome is written back to the source document.
//!
//! Port notes
//! - Python's `db.info["sign_invitations"]` is `PendingInvitations`, passed explicitly.
//! - `db.commit()` in `remind_expiring` is `COMMIT; BEGIN` when the caller's connection is in a transaction
//!   (SAVEPOINT probe, the google_calendar.rs pattern); nothing in autocommit.
//! - routers/signing.py helpers: `_add_audit` (one INSERT) is reproduced here; `_upload_to_minio` and
//!   `_send_signing_invitation` are routers-wave stubs (`later::verification`) behind `seams`, which tests
//!   replace as Python's tests monkeypatch them.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Duration, Utc};
use granary_compat::pyfmt::strftime;
use granary_compat::pyjson::PyDateTime;
use granary_compat::text::pystr;
use granary_compat::text::utils::add_business_days;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::auth::User;
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::hr::{Applicant, OfferLetter};
use granary_db::rows::signing::{SignDocument, SignSigner};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::adapters::{get_adapter, Entity};
use crate::approvals::engine::{self, Policy};
use crate::approvals::routing::{owner_user_ids, role_holder_ids};
use crate::verification::render;
use crate::verification::service as verification;

pub const SIGNING_FLOWS: [&str; 3] = ["counterparty", "internal", "internal_then_counterparty"];
const _WITH_INTERNAL: [&str; 2] = ["internal", "internal_then_counterparty"];
const _WITH_COUNTERPARTY: [&str; 2] = ["counterparty", "internal_then_counterparty"];
pub const OPEN_STATUSES: [&str; 2] = ["draft", "pending"];

const EM_DASH: &str = "-";

fn _now() -> DateTime<Utc> {
    let n = Utc::now();
    DateTime::from_timestamp_micros(n.timestamp_micros()).unwrap_or(n)
}

fn fmt_day(dt: &DateTime<Utc>) -> String {
    strftime(&PyDateTime::from_chrono_utc(dt), "%d %b %Y")
}

fn iso(dt: Option<&DateTime<Utc>>) -> Value {
    match dt {
        Some(d) => Value::String(PyDateTime::from_chrono_utc(d).isoformat()),
        None => Value::Null,
    }
}

/// routers/signing.py `_add_audit(db, document_id, action, description=None, signer_id=None, ...)`.
async fn _add_audit(
    db: &mut PgConnection,
    document_id: Uuid,
    action: &str,
    description: Option<&str>,
    signer_id: Option<Uuid>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO sign_audit_logs (id, document_id, signer_id, action, description, ip_address, user_agent) \
         VALUES ($1, $2, $3, $4, $5, NULL, NULL)",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind(signer_id)
    .bind(action)
    .bind(description)
    .execute(&mut *db)
    .await?;
    Ok(())
}

async fn get_user(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<User>> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

async fn get_applicant(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Applicant>> {
    Ok(sqlx::query_as("SELECT * FROM applicants WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

async fn set_applicant_stage(
    db: &mut PgConnection,
    applicant_id: Uuid,
    stage: &str,
) -> ApiResult<()> {
    if get_applicant(db, applicant_id).await?.is_some() {
        sqlx::query("UPDATE applicants SET stage = $2, updated_at = now() WHERE id = $1")
            .bind(applicant_id)
            .bind(stage)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}

fn parse_py_uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s.trim()).ok()
}

/// `_internal_signer(db, policy)`. `None` when the first id's user row is gone (Python's `db.get` None).
async fn _internal_signer(db: &mut PgConnection, policy: &Policy) -> ApiResult<Option<User>> {
    let role = policy
        .row
        .signer_role
        .clone()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "owner".into());
    let ids = if role == "owner" {
        owner_user_ids(db).await?
    } else {
        role_holder_ids(db, &role, None).await?
    };
    if ids.is_empty() {
        return Err(ApiError::http(
            400,
            format!("No active user holds the signer role '{role}'"),
        ));
    }
    let Some(id) = parse_py_uuid(&ids[0]) else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "badly formed hexadecimal UUID string"
        )));
    };
    get_user(db, id).await
}

async fn _counterparty(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &Entity,
) -> ApiResult<(String, String)> {
    match (entity_type, entity) {
        ("sales_contract", Entity::SalesContract(c)) => {
            let contact: Option<Contact> = match c.customer_contact_id {
                Some(id) => {
                    sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&mut *db)
                        .await?
                }
                None => None,
            };
            if let Some(contact) = contact {
                if let Some(email) = contact.email.clone().filter(|e| !e.is_empty()) {
                    let parts: Vec<&str> = [
                        Some(contact.first_name.as_str()),
                        contact.last_name.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|p| !p.is_empty())
                    .collect();
                    let name = parts.join(" ");
                    return Ok((if name.is_empty() { email.clone() } else { name }, email));
                }
            }
            let org: Option<Organization> = match c.customer_org_id {
                Some(id) => {
                    sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&mut *db)
                        .await?
                }
                None => None,
            };
            if let Some(org) = org {
                if let Some(email) = org.email.clone().filter(|e| !e.is_empty()) {
                    return Ok((org.name.clone(), email));
                }
            }
            Err(ApiError::http(
                400,
                "The contract's customer has no contact email to send the signature request to",
            ))
        }
        ("offer_letter", Entity::OfferLetter(o)) => {
            if let Some(a) = get_applicant(db, o.applicant_id).await? {
                if let Some(email) = a.email.clone().filter(|e| !e.is_empty()) {
                    return Ok((a.name.clone(), email));
                }
            }
            Err(ApiError::http(
                400,
                "The candidate has no email address to send the offer to",
            ))
        }
        _ => Err(ApiError::http(
            400,
            format!(
                "{} documents have no counterparty signer",
                get_adapter(entity_type)?.label
            ),
        )),
    }
}

/// Where a signature field goes (`_place_fields` values; percentages of the page).
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub page_number: i32,
    pub x: Decimal,
    pub y: Decimal,
    pub width: Decimal,
    pub height: Decimal,
}

/// Python's `round(x, 2)` on a float, as the NUMERIC(8,2) column receives it.
fn round2(x: f64) -> Decimal {
    // Rust's fixed-precision formatting rounds the exact binary value half-to-even, as CPython's
    // round() does; the 2-place decimal string is what the float's repr would print.
    format!("{x:.2}").parse().unwrap_or_default()
}

/// Signature field per signing order (1-based, in `anchors` order), positioned on the template's hidden
/// anchors (fallback: last page).
pub fn _place_fields(
    pdf_bytes: &[u8],
    anchors: &[&str],
) -> ApiResult<std::collections::BTreeMap<i32, Placement>> {
    use crate::pdftools::Document;
    let err = |e: crate::pdftools::PdfError| ApiError::internal(anyhow::anyhow!(e.0));
    let pdf = Document::open(pdf_bytes).map_err(err)?;
    let mut placed = std::collections::BTreeMap::new();
    let count = pdf.page_count();
    for (i, anchor) in anchors.iter().enumerate() {
        let order = i as i32 + 1;
        let mut found = None;
        for index in (0..count).rev() {
            let hits = pdf.search_for(index, anchor).map_err(err)?;
            if let Some(r) = hits.first() {
                let rect = pdf.page_rect(index).map_err(err)?;
                let (w, h) = (rect.width(), rect.height());
                found = Some(Placement {
                    page_number: index as i32 + 1,
                    x: round2(r.x0 / w * 100.0),
                    y: round2(r.y0 / h * 100.0),
                    width: round2(170f64.min(w - r.x0) / w * 100.0),
                    height: round2(46.0 / h * 100.0),
                });
                break;
            }
        }
        placed.insert(
            order,
            found.unwrap_or(Placement {
                page_number: count as i32,
                x: Decimal::from(8 + (order - 1) * 48),
                y: Decimal::from(72),
                width: Decimal::from(36),
                height: Decimal::from(8),
            }),
        );
    }
    Ok(placed)
}

/// Python's `db.info["sign_invitations"]`: (doc_id, signer_id) pairs to email after commit.
#[derive(Debug, Default, Clone)]
pub struct PendingInvitations(pub Vec<(String, String)>);

async fn insert_signer(
    db: &mut PgConnection,
    doc_id: Uuid,
    name: &str,
    email: &str,
    sign_order: i32,
    user_id: Option<Uuid>,
    signer_type: &str,
) -> ApiResult<SignSigner> {
    Ok(sqlx::query_as(
        "INSERT INTO sign_signers (id, document_id, name, email, sign_order, role, token, status, user_id, signer_type) \
         VALUES ($1, $2, $3, $4, $5, 'signer', $6, 'pending', $7, $8) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(doc_id)
    .bind(name)
    .bind(email)
    .bind(sign_order)
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(signer_type)
    .fetch_one(&mut *db)
    .await?)
}

/// `start_from_document`. Returns the new sign_documents row. The first signer's invitation is queued in
/// `invites` (Python: `db.info["sign_invitations"]`); call `dispatch_invitations` after commit.
pub async fn start_from_document(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
    user: &AuthUser,
    message: Option<&str>,
    invites: &mut PendingInvitations,
) -> ApiResult<SignDocument> {
    let adapter = get_adapter(entity_type)?;
    let policy = engine::get_policy(db, entity_type).await?;
    let Some(policy) = policy.filter(|p| SIGNING_FLOWS.contains(&p.row.signature_flow.as_str()))
    else {
        return Err(ApiError::http(
            400,
            format!("{}s are not sent for e-signature", adapter.label),
        ));
    };
    let Some(mut entity) = adapter.load(db, entity_id).await? else {
        return Err(ApiError::http(404, format!("{} not found", adapter.label)));
    };
    let existing: Option<SignDocument> = sqlx::query_as(
        "SELECT * FROM sign_documents WHERE source_type = $1 AND source_id = $2 AND status IN ('draft', 'pending') \
         AND is_deleted = false LIMIT 1",
    )
    .bind(entity_type)
    .bind(entity.id())
    .fetch_optional(&mut *db)
    .await?;
    if let Some(existing) = existing {
        return Err(ApiError::http_json(
            409,
            json!({"code": "signature_in_progress", "sign_document_id": existing.id.to_string(),
                   "message": format!("This {} is already out for signature", pystr::lower(adapter.label))}),
        ));
    }

    // Resolve every signer before rendering, so a missing email fails fast.
    let flow = policy.row.signature_flow.as_str();
    let signer_user = if _WITH_INTERNAL.contains(&flow) {
        _internal_signer(db, &policy).await?
    } else {
        None
    };
    let counterparty = if _WITH_COUNTERPARTY.contains(&flow) {
        Some(_counterparty(db, entity_type, &entity).await?)
    } else {
        None
    };

    let issued = verification::issue(db, entity_type, entity_id, user).await?;
    let pdf = verification::get_pdf(issued.pdf_object_key.as_deref().unwrap_or_default())
        .await
        .map_err(ApiError::internal)?;
    let now = _now();
    let deadline_days = policy.row.sign_deadline_days.unwrap_or(0);
    let expires_at = if counterparty.is_some() && deadline_days != 0 {
        Some(add_business_days(now, deadline_days as i64))
    } else {
        None
    };
    let title = issued
        .document_title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| adapter.document_label(&entity));
    let doc_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sign_documents (id, title, status, source_type, source_id, document_hash, signature_level, \
         jurisdictions, message, expires_at, created_by_id, is_deleted) \
         VALUES ($1, $2, 'pending', $3, $4, $5, 'ses', 'US,EU,ID', $6, $7, $8, false)",
    )
    .bind(doc_id)
    .bind(&title)
    .bind(entity_type)
    .bind(entity.id())
    .bind(&issued.document_hash)
    .bind(message)
    .bind(expires_at)
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    let url = upload_to_minio(
        pdf.clone(),
        format!("originals/{doc_id}.pdf"),
        "application/pdf".into(),
    )
    .await?;
    sqlx::query(
        "UPDATE sign_documents SET original_file_url = $2, updated_at = now() WHERE id = $1",
    )
    .bind(doc_id)
    .bind(&url)
    .execute(&mut *db)
    .await?;

    // Our signatory prints on [[SIG-1]], the counterparty on [[SIG-2]] (see render.SIGNATURE_ANCHORS).
    let mut signers: Vec<SignSigner> = vec![];
    let mut anchors: Vec<&str> = vec![];
    if let Some(u) = &signer_user {
        signers.push(
            insert_signer(
                db,
                doc_id,
                &u.name,
                &pystr::lower(&u.email),
                1,
                Some(u.id),
                "internal",
            )
            .await?,
        );
        anchors.push(render::SIGNATURE_ANCHORS[0]);
    }
    if let Some((name, email)) = &counterparty {
        let order = signers.len() as i32 + 1;
        signers.push(
            insert_signer(
                db,
                doc_id,
                name,
                &pystr::lower(email),
                order,
                None,
                "external",
            )
            .await?,
        );
        anchors.push(render::SIGNATURE_ANCHORS[1]);
    }
    let placed = _place_fields(&pdf, &anchors)?;
    for s in &signers {
        let p = &placed[&s.sign_order];
        sqlx::query(
            "INSERT INTO sign_fields (id, document_id, signer_id, field_type, page_number, x, y, width, height, \
             is_required, label) VALUES ($1, $2, $3, 'signature', $4, $5, $6, $7, $8, true, $9)",
        )
        .bind(Uuid::new_v4())
        .bind(doc_id)
        .bind(s.id)
        .bind(p.page_number)
        .bind(p.x)
        .bind(p.y)
        .bind(p.width)
        .bind(p.height)
        .bind(format!("Signature {EM_DASH} {}", s.name))
        .execute(&mut *db)
        .await?;
    }
    sqlx::query(
        "UPDATE document_verifications SET sign_document_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(issued.id)
    .bind(doc_id)
    .execute(&mut *db)
    .await?;

    let number = adapter.number(&entity);
    let label = pystr::lower(adapter.label);
    _add_audit(
        db,
        doc_id,
        "document_created",
        Some(&format!(
            "Created from approved {label} {number} by {}",
            user.email
        )),
        None,
    )
    .await?;
    _add_audit(
        db,
        doc_id,
        "document_sent",
        Some(&format!("Sent for signing by {}", user.email)),
        None,
    )
    .await?;
    // signers[0]: IndexError (a 500) when the flow resolved no signer, as in Python.
    let first = signers
        .first()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("list index out of range")))?;
    sqlx::query(
        "UPDATE sign_signers SET status = 'sent', sent_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(first.id)
    .bind(now)
    .execute(&mut *db)
    .await?;
    _add_audit(
        db,
        doc_id,
        "sent_to_signer",
        Some(&format!("Invitation sent to {}", first.email)),
        Some(first.id),
    )
    .await?;
    invites.0.push((doc_id.to_string(), first.id.to_string()));

    match &mut entity {
        Entity::SalesContract(c)
            if entity_type == "sales_contract" && c.status.as_deref() == Some("draft") =>
        {
            sqlx::query("UPDATE contracts SET status = 'sent_for_signing', updated_at = now() WHERE id = $1")
                .bind(c.id)
                .execute(&mut *db)
                .await?;
        }
        Entity::OfferLetter(o)
            if entity_type == "offer_letter" && o.status.as_deref() == Some("draft") =>
        {
            sqlx::query("UPDATE offer_letters SET status = 'sent', sent_at = $2, updated_at = now() WHERE id = $1")
                .bind(o.id)
                .bind(now)
                .execute(&mut *db)
                .await?;
            set_applicant_stage(db, o.applicant_id, "offer_sent").await?;
        }
        _ => {}
    }
    Ok(sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
        .bind(doc_id)
        .fetch_one(&mut *db)
        .await?)
}

/// Call after commit: email the signers queued by start_from_document (background tasks).
pub fn dispatch_invitations(invites: &mut PendingInvitations) {
    for (doc_id, signer_id) in std::mem::take(&mut invites.0) {
        tokio::spawn(async move {
            let _ = send_signing_invitation(doc_id, signer_id).await;
        });
    }
}

/// Write the executed signature back to the source document. `signers` are `doc.signers`.
pub async fn on_sign_completed(
    db: &mut PgConnection,
    doc: &SignDocument,
    signers: &[SignSigner],
) -> ApiResult<()> {
    let (Some(source_type), Some(source_id)) = (
        doc.source_type.as_deref().filter(|s| !s.is_empty()),
        doc.source_id,
    ) else {
        return Ok(());
    };
    let loaded = match get_adapter(source_type) {
        Ok(a) => a.load(db, &source_id.to_string()).await,
        Err(e) => Err(e),
    };
    let entity = match loaded {
        Ok(Some(e)) => e,
        Ok(None) => return Ok(()),
        Err(ApiError::Http { .. }) => return Ok(()),
        Err(e) => return Err(e),
    };
    let now = _now();
    let mut ordered: Vec<&SignSigner> = signers
        .iter()
        .filter(|s| s.role.as_deref() == Some("signer"))
        .collect();
    ordered.sort_by_key(|s| s.sign_order);
    let internal = ordered
        .iter()
        .find(|s| s.signer_type.as_deref() == Some("internal"))
        .copied();
    let external = ordered
        .iter()
        .find(|s| s.signer_type.as_deref() != Some("internal"))
        .copied();
    match (source_type, &entity) {
        ("sales_contract", Entity::SalesContract(c)) => {
            let mut customer_name = c.signed_by_customer_name.clone();
            let mut granary_name = c.signed_by_company_name.clone();
            let mut granary_id = c.signed_by_company_id;
            if let Some(ext) = external {
                customer_name = Some(ext.name.clone());
            }
            if let Some(int) = internal {
                granary_name = Some(int.name.clone());
                granary_id = int.user_id;
            } else {
                // Counterparty-only flow: our side was signed by approving the contract.
                if let Some(approver) = _final_approver(db, source_type, c.id).await? {
                    granary_name = Some(approver.name.clone());
                    granary_id = Some(approver.id);
                }
            }
            sqlx::query(
                "UPDATE contracts SET status = 'signed', signed_at = $2, signed_by_customer_name = $3, \
                 signed_by_company_name = $4, signed_by_company_id = $5, updated_at = now() WHERE id = $1",
            )
            .bind(c.id)
            .bind(now)
            .bind(customer_name)
            .bind(granary_name)
            .bind(granary_id)
            .execute(&mut *db)
            .await?;
        }
        ("offer_letter", Entity::OfferLetter(o))
            if matches!(o.status.as_deref(), Some("draft" | "sent" | "countered")) =>
        {
            let sent_at = o.sent_at.unwrap_or(doc.created_at);
            let notes = o
                .candidate_response_notes
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Accepted by electronic signature".into());
            sqlx::query(
                "UPDATE offer_letters SET status = 'accepted', sent_at = $2, responded_at = $3, \
                 candidate_response_notes = $4, updated_at = now() WHERE id = $1",
            )
            .bind(o.id)
            .bind(sent_at)
            .bind(now)
            .bind(notes)
            .execute(&mut *db)
            .await?;
            set_applicant_stage(db, o.applicant_id, "offer_accepted").await?;
        }
        ("quotation", Entity::Quotation(q)) => {
            sqlx::query("UPDATE quotations SET signed_at = $2, updated_at = now() WHERE id = $1")
                .bind(q.row.id)
                .bind(now)
                .execute(&mut *db)
                .await?;
        }
        _ => {}
    }
    tracing::info!(source_type, source_id = %source_id, "signature_written_back");
    Ok(())
}

async fn _final_approver(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> ApiResult<Option<User>> {
    let request = engine::latest_request(db, entity_type, entity_id).await?;
    let Some(request) = request.filter(|r| r.status == "approved") else {
        return Ok(None);
    };
    let truthy = |v: Option<&Value>| v.map(crate::whatsapp::py_truthy).unwrap_or(false);
    let steps = request.steps.as_array().cloned().unwrap_or_default();
    let decided: Vec<&Value> = steps
        .iter()
        .filter(|st| {
            st.get("decision").and_then(|d| d.as_str()) == Some("approved")
                && truthy(st.get("decided_by_id"))
                && !truthy(st.get("is_notify_only"))
        })
        .collect();
    let Some(last) = decided.last() else {
        return Ok(None);
    };
    let raw = last
        .get("decided_by_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let Some(id) = parse_py_uuid(raw) else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "badly formed hexadecimal UUID string"
        )));
    };
    get_user(db, id).await
}

pub async fn on_sign_declined(
    db: &mut PgConnection,
    doc: &SignDocument,
    signer: &SignSigner,
) -> ApiResult<()> {
    if doc.source_type.as_deref() != Some("offer_letter")
        || signer.signer_type.as_deref() == Some("internal")
    {
        return Ok(());
    }
    let Some(source_id) = doc.source_id else {
        return Ok(());
    };
    let offer: Option<OfferLetter> = sqlx::query_as("SELECT * FROM offer_letters WHERE id = $1")
        .bind(source_id)
        .fetch_optional(&mut *db)
        .await?;
    let Some(offer) = offer.filter(|o| matches!(o.status.as_deref(), Some("sent" | "countered")))
    else {
        return Ok(());
    };
    let notes = signer
        .declined_reason
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Declined at signing".into());
    sqlx::query(
        "UPDATE offer_letters SET status = 'declined', responded_at = $2, candidate_response_notes = $3, \
         updated_at = now() WHERE id = $1",
    )
    .bind(offer.id)
    .bind(_now())
    .bind(notes)
    .execute(&mut *db)
    .await?;
    set_applicant_stage(db, offer.applicant_id, "offer_rejected").await
}

async fn _deadline_limits(
    db: &mut PgConnection,
    doc: &SignDocument,
) -> ApiResult<(Option<Policy>, Option<DateTime<Utc>>)> {
    let policy = match doc.source_type.as_deref().filter(|s| !s.is_empty()) {
        Some(t) => engine::get_policy(db, t).await?,
        None => None,
    };
    let Some(policy) = policy else {
        return Ok((None, None));
    };
    let days = policy.row.sign_deadline_days.unwrap_or(0);
    if days == 0 || doc.expires_at.is_none() {
        return Ok((None, None));
    }
    let base = add_business_days(doc.created_at, days as i64);
    let limit = add_business_days(base, policy.row.max_extension_days.unwrap_or(0) as i64);
    Ok((Some(policy), Some(limit)))
}

/// `extend_deadline(db, doc, days, user)`: updates `doc` in place (and the row).
pub async fn extend_deadline(
    db: &mut PgConnection,
    doc: &mut SignDocument,
    days: i64,
    user: &AuthUser,
) -> ApiResult<()> {
    if doc.status.as_deref() != Some("pending") {
        let status = doc.status.clone().unwrap_or_else(|| "None".into());
        return Err(ApiError::http(
            400,
            format!("A {status} signature request cannot be extended"),
        ));
    }
    let (policy, limit) = _deadline_limits(db, doc).await?;
    let (Some(policy), Some(limit)) = (policy, limit) else {
        return Err(ApiError::http(
            400,
            "This signature request has no deadline to extend",
        ));
    };
    let now = _now();
    let expires = doc.expires_at.unwrap_or(now);
    let start = if now > expires { now } else { expires };
    let new_deadline = add_business_days(start, days);
    if new_deadline > limit + Duration::minutes(1) {
        return Err(ApiError::http(
            400,
            format!(
                "The deadline can be extended by at most {} business days (latest {})",
                policy.row.max_extension_days.unwrap_or(0),
                fmt_day(&limit)
            ),
        ));
    }
    doc.expires_at = Some(new_deadline);
    sqlx::query("UPDATE sign_documents SET expires_at = $2, updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .bind(new_deadline)
        .execute(&mut *db)
        .await?;
    _add_audit(
        db,
        doc.id,
        "deadline_extended",
        Some(&format!(
            "Deadline extended to {} by {}",
            fmt_day(&new_deadline),
            user.email
        )),
        None,
    )
    .await
}

async fn signers_of(db: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<SignSigner>> {
    // `doc.signers` (selectinload): the relationship has order_by="SignSigner.sign_order", so SQLAlchemy
    // emits exactly ORDER BY sign_signers.sign_order (ties in the database's order, as in Python).
    Ok(sqlx::query_as(
        "SELECT * FROM sign_signers WHERE sign_signers.document_id = $1 ORDER BY sign_signers.sign_order",
    )
    .bind(doc_id)
    .fetch_all(&mut *db)
    .await?)
}

pub async fn status_for_document(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
) -> ApiResult<Value> {
    let Some(uid) = parse_py_uuid(entity_id) else {
        return Ok(json!({"sign_document": null}));
    };
    let doc: Option<SignDocument> = sqlx::query_as(
        "SELECT * FROM sign_documents WHERE source_type = $1 AND source_id = $2 AND is_deleted = false \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(entity_type)
    .bind(uid)
    .fetch_optional(&mut *db)
    .await?;
    let Some(doc) = doc else {
        return Ok(json!({"sign_document": null}));
    };
    let (_, limit) = _deadline_limits(db, &doc).await?;
    let mut signers = signers_of(db, doc.id).await?;
    signers.sort_by_key(|s| s.sign_order);
    let can_extend = doc.status.as_deref() == Some("pending")
        && limit.is_some()
        && doc.expires_at.is_some()
        && doc.expires_at < limit;
    let signer_rows: Vec<Value> = signers
        .iter()
        .filter(|s| s.role.as_deref() == Some("signer"))
        .map(|s| {
            json!({"name": s.name, "email": s.email, "sign_order": s.sign_order, "status": s.status,
                   "signer_type": s.signer_type, "signed_at": iso(s.signed_at.as_ref())})
        })
        .collect();
    Ok(json!({"sign_document": {
        "id": doc.id.to_string(), "title": doc.title, "status": doc.status,
        "created_at": iso(Some(&doc.created_at)), "expires_at": iso(doc.expires_at.as_ref()),
        "completed_at": iso(doc.completed_at.as_ref()),
        "latest_deadline": iso(limit.as_ref()),
        "can_extend": can_extend,
        "signers": signer_rows,
    }}))
}

/// Re-invite signers whose deadline is near (sign.reminder_window_hours), at most once per
/// sign.reminder_interval_hours per signer.
pub async fn remind_expiring(db: &mut PgConnection) -> ApiResult<i64> {
    use granary_core::settings_store::require_int;
    let window = Duration::hours(
        require_int(
            db,
            "sign.reminder_window_hours",
            "Approvals & signing \u{2192} E-signing",
        )
        .await?,
    );
    let interval = Duration::hours(
        require_int(
            db,
            "sign.reminder_interval_hours",
            "Approvals & signing \u{2192} E-signing",
        )
        .await?,
    );
    let now = _now();
    let docs: Vec<SignDocument> = sqlx::query_as(
        "SELECT * FROM sign_documents WHERE status = 'pending' AND source_type IS NOT NULL AND expires_at IS NOT NULL \
         AND expires_at > $1 AND expires_at < $2 AND is_deleted = false",
    )
    .bind(now)
    .bind(now + window)
    .fetch_all(&mut *db)
    .await?;
    let mut sent = 0i64;
    for doc in docs {
        for signer in signers_of(db, doc.id).await? {
            if signer.role.as_deref() != Some("signer")
                || !matches!(signer.status.as_deref(), Some("sent" | "viewed"))
            {
                continue;
            }
            let recent: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM sign_audit_logs WHERE document_id = $1 AND signer_id = $2 AND action = 'reminder_sent' \
                 AND created_at > $3 LIMIT 1",
            )
            .bind(doc.id)
            .bind(signer.id)
            .bind(now - interval)
            .fetch_optional(&mut *db)
            .await?;
            if recent.is_some() {
                continue;
            }
            send_signing_invitation(doc.id.to_string(), signer.id.to_string()).await?;
            _add_audit(
                db,
                doc.id,
                "reminder_sent",
                Some(&format!("Deadline reminder sent to {}", signer.email)),
                Some(signer.id),
            )
            .await?;
            // Commit per send: the email has gone, so its audit row must survive a later failure
            // (otherwise the next run re-sends, having no record of this one).
            commit(db).await?;
            sent += 1;
        }
    }
    Ok(sent)
}

/// `await db.commit()` on the caller's connection (the google_calendar.rs pattern): inside a transaction
/// COMMIT; BEGIN, in autocommit nothing.
async fn commit(db: &mut PgConnection) -> ApiResult<()> {
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

async fn upload_to_minio(
    data: Vec<u8>,
    object_name: String,
    content_type: String,
) -> ApiResult<String> {
    if let Some(f) = seams::upload_to_minio() {
        return f(data, object_name, content_type).await;
    }
    crate::later::verification::routers_signing_upload_to_minio(data, object_name, content_type)
        .await
}

async fn send_signing_invitation(doc_id: String, signer_id: String) -> ApiResult<()> {
    if let Some(f) = seams::send_signing_invitation() {
        return f(doc_id, signer_id).await;
    }
    crate::later::verification::routers_signing_send_signing_invitation(doc_id, signer_id).await
}

/// Test seams for the routers/signing.py helpers Python's tests monkeypatch.
pub mod seams {
    use super::*;

    pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
    pub type UploadFn =
        Arc<dyn Fn(Vec<u8>, String, String) -> BoxFut<ApiResult<String>> + Send + Sync>;
    pub type InviteFn = Arc<dyn Fn(String, String) -> BoxFut<ApiResult<()>> + Send + Sync>;

    static UPLOAD: RwLock<Option<UploadFn>> = RwLock::new(None);
    static INVITE: RwLock<Option<InviteFn>> = RwLock::new(None);

    pub fn set_upload_to_minio(f: Option<UploadFn>) {
        *UPLOAD.write().unwrap() = f;
    }
    pub fn set_send_signing_invitation(f: Option<InviteFn>) {
        *INVITE.write().unwrap() = f;
    }
    pub(super) fn upload_to_minio() -> Option<UploadFn> {
        UPLOAD.read().unwrap().clone()
    }
    pub(super) fn send_signing_invitation() -> Option<InviteFn> {
        INVITE.read().unwrap().clone()
    }
}
