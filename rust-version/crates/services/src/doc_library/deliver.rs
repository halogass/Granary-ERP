//! doclib A2: copies of a library document for people outside the company (Nary's sends, staff sending from the
//! ERP, the partner portal). Every copy is stamped with the customer footer and its own fingerprint, and
//! logged in library_downloads.
//!
//! Hard rules in code (whoever calls): never a price_list document, never a document without a customer or
//! partner audience, only the live version, and a partner-only document only to a partner account.

use granary_core::errors::{ApiError, ApiResult};
use sqlx::PgConnection;
use uuid::Uuid;

use super::fingerprint::{self, Issued, Kind, Who};
use super::{load_document, read_file, Document, Version, STATUS_LIVE};
use crate::nary::outbound_files::FileBlob;

/// Who sends the copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    /// Nary (an applied card); the staff member who confirmed it, when there is one.
    Nary { confirmed_by: Option<Uuid> },
    /// A staff member sending from the ERP.
    Staff(Uuid),
}

/// Who receives it.
#[derive(Debug, Clone, Default)]
pub struct Recipient {
    pub company: Option<String>,
    pub name: Option<String>,
    /// set when the recipient is (a contact of) a partner account
    pub partner_account_id: Option<Uuid>,
}

/// A stamped copy and its fingerprint.
#[derive(Debug, Clone)]
pub struct Delivered {
    pub file: FileBlob,
    pub fingerprint: String,
    pub footer_text: String,
}

fn refuse(detail: impl Into<String>) -> ApiError {
    ApiError::http(403, detail.into())
}

/// The rules every external copy must meet.
pub fn check_sendable(doc: &Document, version: &Version, to_partner: bool) -> ApiResult<()> {
    if doc.is_deleted {
        return Err(ApiError::http(404, "Document not found"));
    }
    if doc.kind == "price_list" {
        return Err(refuse(
            "The internal price list is never sent outside the company",
        ));
    }
    if !doc.is_external() {
        return Err(refuse(format!(
            "\"{}\" is an internal document; it can't be sent to a customer or partner",
            doc.title
        )));
    }
    if !doc.has_audience("customer") && !to_partner {
        return Err(refuse(format!(
            "\"{}\" is for partners only; it can only go to a partner account",
            doc.title
        )));
    }
    if version.document_id != doc.id
        || version.status != STATUS_LIVE
        || doc.current_version_id != Some(version.id)
    {
        return Err(refuse(
            "Only the current live version of a document can be sent",
        ));
    }
    Ok(())
}

/// `customer_copy(db, version, recipient_company, channel, sender)`: the live version stamped with the
/// customer footer for that company, logged as nary_send (Nary) or customer_send (staff).
pub async fn customer_copy(
    db: &mut PgConnection,
    version: &Version,
    recipient_company: Option<&str>,
    channel: &str,
    sender: Sender,
) -> ApiResult<FileBlob> {
    let r = Recipient {
        company: recipient_company.map(str::to_string),
        ..Default::default()
    };
    Ok(customer_copy_for(db, version, &r, channel, sender)
        .await?
        .file)
}

/// `customer_copy` with the full recipient (name, partner account) and the fingerprint back.
pub async fn customer_copy_for(
    db: &mut PgConnection,
    version: &Version,
    recipient: &Recipient,
    channel: &str,
    sender: Sender,
) -> ApiResult<Delivered> {
    let doc = load_document(db, version.document_id)
        .await?
        .ok_or_else(|| ApiError::http(404, "Document not found"))?;
    check_sendable(&doc, version, recipient.partner_account_id.is_some())?;
    let (kind, user_id) = match sender {
        Sender::Nary { confirmed_by } => (Kind::NarySend, confirmed_by),
        Sender::Staff(u) => (Kind::CustomerSend, Some(u)),
    };
    let who = Who {
        user_id,
        name: recipient.name.clone(),
        company: recipient.company.clone(),
        partner_account_id: recipient.partner_account_id,
        recipient_name: recipient.name.clone(),
        recipient_company: recipient.company.clone(),
        channel: Some(channel.to_string()),
        ip: None,
        user_agent: None,
    };
    let bytes = read_file(version).await?;
    let (data, issued) =
        fingerprint::stamped_copy(db, kind, version, &who, &bytes, &version.file_mime).await?;
    Ok(delivered(version, data, issued))
}

/// The partner portal download: the live version stamped with the customer footer for the partner's
/// company, logged as partner_download.
pub async fn partner_copy(
    db: &mut PgConnection,
    version: &Version,
    partner_account_id: Uuid,
    partner_company: &str,
    user_id: Uuid,
    ip: Option<String>,
    user_agent: Option<String>,
) -> ApiResult<Delivered> {
    let doc = load_document(db, version.document_id)
        .await?
        .ok_or_else(|| ApiError::http(404, "Document not found"))?;
    if !doc.has_audience("partner") {
        return Err(ApiError::http(404, "Document not found"));
    }
    check_sendable(&doc, version, true)?;
    let who = Who {
        user_id: Some(user_id),
        company: Some(partner_company.to_string()),
        partner_account_id: Some(partner_account_id),
        recipient_company: Some(partner_company.to_string()),
        channel: Some("portal".into()),
        ip,
        user_agent,
        ..Default::default()
    };
    let bytes = read_file(version).await?;
    let (data, issued) = fingerprint::stamped_copy(
        db,
        Kind::PartnerDownload,
        version,
        &who,
        &bytes,
        &version.file_mime,
    )
    .await?;
    Ok(delivered(version, data, issued))
}

fn delivered(version: &Version, data: Vec<u8>, issued: Issued) -> Delivered {
    Delivered {
        file: FileBlob {
            filename: version.file_name.clone(),
            mime: version.file_mime.clone(),
            data,
        },
        fingerprint: issued.id,
        footer_text: issued.footer_text,
    }
}
