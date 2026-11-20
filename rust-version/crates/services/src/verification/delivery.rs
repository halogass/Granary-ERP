//! Port of backend/app/services/verification/delivery.py.
//!
//! Send an approved document to its client or vendor: the QR-verified PDF, by email or WhatsApp.
//!
//! Nothing leaves before approval: `send` runs the approval gate first (an unapproved document is
//! submitted and the send refused with 409), then delivers the active verified PDF (issuing one if the
//! document has none) through the CRM mailbox or the WhatsApp service, and records the send.
//!
//! A send is not repeated by accident: while one is in flight a second identical one is refused, and
//! the same document to the same recipient within 15 minutes needs an explicit "send again" (`force`).
//!
//! Port notes
//! - The SQLAlchemy entity is `approvals::adapters::Entity`; every attribute Python set on it (flushed at
//!   commit) is written here with an UPDATE at the same point and mirrored on the struct.
//! - Python's monkeypatch targets (`delivery._redis`, `delivery._send_email`, `delivery._verified_pdf`,
//!   and the routers' `_emit_quotation`) are seams in `seams`: tests install fakes there.
//! - `routers/whatsapp.py` `_wa_request` / `_raise_wa_error` (wave 4) are reproduced privately here on the
//!   `crate::whatsapp` httpx look-alike.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::finance::Invoice;
use granary_db::rows::sales::{Quotation, QuotationItem};
use granary_db::rows::vendor::Vendor;
use granary_db::rows::verification::DocumentVerification;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::adapters::{get_adapter, DocumentAdapter, Entity, QuotationDoc};
use crate::approvals::engine;
use crate::verification::render;
use crate::verification::service as verification;

/// entity_type to the permission to send it.
pub const SENDABLE: &[(&str, &str)] = &[
    ("quotation", "sales:send_quotation"),
    ("invoice", "finance:send_invoice"),
    ("purchase_order", "vendor_mgmt:create_purchase_order"),
    ("work_order", "vendor_mgmt:create_work_order"),
    ("sales_document", "sales:send_quotation"), // a Solution Proposal, sent with its quotation(s)
];
pub const CHANNELS: [&str; 2] = ["email", "whatsapp"];
pub const ATTACHABLE: [&str; 3] = ["quotation", "invoice", "sales_document"];
pub const RECENT_SEND_SECONDS: u64 = 15 * 60;
pub const INFLIGHT_SECONDS: u64 = 180;

const EM_DASH: &str = "-";

fn sendable(entity_type: &str) -> Option<&'static str> {
    SENDABLE
        .iter()
        .find(|(t, _)| *t == entity_type)
        .map(|(_, p)| *p)
}

fn now() -> DateTime<Utc> {
    let n = Utc::now();
    DateTime::from_timestamp_micros(n.timestamp_micros()).unwrap_or(n)
}

/// `str(v)` of a JSON value the way Python prints it (None, True, 1.5, 'a' inside containers...).
fn py_str(v: &Value) -> String {
    str_value(&PyValue::from(v))
}

fn py_truthy(v: Option<&Value>) -> bool {
    v.map(crate::whatsapp::py_truthy).unwrap_or(false)
}

/// `a or b` on optional strings (an empty string is falsy).
fn or_s(a: Option<String>, b: Option<String>) -> Option<String> {
    match a {
        Some(s) if !s.is_empty() => Some(s),
        _ => b.filter(|s| !s.is_empty()).or(None),
    }
}

/// `x or None` on an optional string.
fn nonempty(a: Option<String>) -> Option<String> {
    a.filter(|s| !s.is_empty())
}

/// One attachment dict of `email_attachments` (Python carries the bytes twice, as "content" and "data",
/// and the type twice, as "content_type" and "mime_type").
#[derive(Debug, Clone, PartialEq)]
pub struct EmailAttachment {
    pub filename: String,
    pub content: Vec<u8>,
    pub content_type: String,
    pub mime_type: String,
}

impl EmailAttachment {
    /// `item["data"]`: the same bytes as `content`.
    pub fn data(&self) -> &[u8] {
        &self.content
    }
}

pub fn adapter_for(entity_type: &str) -> ApiResult<&'static DocumentAdapter> {
    if sendable(entity_type).is_none() {
        return Err(ApiError::http(
            400,
            "This document type is not sent from here",
        ));
    }
    get_adapter(entity_type)
}

fn _person(parts: &[Option<&str>]) -> String {
    let joined: Vec<&str> = parts
        .iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .copied()
        .collect();
    pystr::strip(&joined.join(" ")).to_string()
}

async fn get_contact(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<Contact>> {
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

async fn get_org(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<Organization>> {
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// `entity.vendor_id` (purchase and work orders).
fn vendor_id_of(entity: &Entity) -> Option<Uuid> {
    match entity {
        Entity::PurchaseOrder(po) => Some(po.row.vendor_id),
        Entity::WorkOrder(wo) => Some(wo.vendor_id),
        Entity::VendorInvoice(vi) => Some(vi.vendor_id),
        _ => None,
    }
}

fn deal_id_of(entity: &Entity) -> Option<Uuid> {
    match entity {
        Entity::Quotation(q) => q.row.deal_id,
        Entity::Invoice(i) => i.deal_id,
        Entity::SalesDocument(d) => d.deal_id,
        Entity::SalesContract(c) => c.deal_id,
        _ => None,
    }
}

/// Who the document goes to, for the pre-filled (and editable) send dialog. An invoice's own
/// billing email / WhatsApp come first (they also cover customers who aren't in the CRM).
pub async fn recipient_defaults(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &Entity,
) -> ApiResult<Value> {
    let (mut name, mut email, mut whatsapp): (Option<String>, Option<String>, Option<String>) =
        (None, None, None);
    if entity_type == "invoice" {
        if let Entity::Invoice(inv) = entity {
            name = nonempty(inv.customer_contact_person.clone());
            email = nonempty(inv.customer_email.clone());
            whatsapp = nonempty(inv.customer_tel.clone());
        }
    }
    if matches!(entity_type, "quotation" | "invoice" | "sales_document") {
        let (contact_id, org_id) = _customer_ids(entity_type, entity);
        let contact = get_contact(db, contact_id).await?;
        let org = get_org(db, org_id).await?;
        if let Some(c) = &contact {
            name = or_s(
                name,
                nonempty(Some(_person(&[
                    Some(&c.first_name),
                    c.last_name.as_deref(),
                ]))),
            );
            email = or_s(email, c.email.clone());
            whatsapp = or_s(or_s(whatsapp, c.whatsapp.clone()), c.phone.clone());
        }
        let customer_name = match entity {
            Entity::Invoice(inv) => inv.customer_name.clone(),
            _ => None, // getattr(entity, "customer_name", None): only invoices have one
        };
        name = or_s(
            or_s(name, customer_name),
            org.as_ref().map(|o| o.name.clone()),
        );
        email = or_s(email, org.as_ref().and_then(|o| o.email.clone()));
        whatsapp = or_s(whatsapp, org.as_ref().and_then(|o| o.phone.clone()));
    } else {
        let vendor: Option<Vendor> = match vendor_id_of(entity) {
            Some(id) => {
                sqlx::query_as("SELECT * FROM vendors WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *db)
                    .await?
            }
            None => None,
        };
        if let Some(v) = vendor {
            name = or_s(v.contact_name.clone(), Some(v.name.clone()));
            email = v.contact_email.clone();
            whatsapp = or_s(v.contact_whatsapp.clone(), v.contact_phone.clone());
        }
    }
    Ok(
        json!({"name": name.unwrap_or_default(), "email": email.unwrap_or_default(),
              "whatsapp": whatsapp.unwrap_or_default()}),
    )
}

fn _customer_ids(entity_type: &str, entity: &Entity) -> (Option<Uuid>, Option<Uuid>) {
    match entity {
        Entity::SalesDocument(d) if entity_type == "sales_document" => {
            (d.contact_id, d.organization_id)
        }
        Entity::Quotation(q) => (q.row.customer_contact_id, q.row.customer_org_id),
        Entity::Invoice(i) => (i.customer_contact_id, i.customer_org_id),
        Entity::SalesContract(c) => (c.customer_contact_id, c.customer_org_id),
        _ => (None, None),
    }
}

/// The quotations a proposal presents (sent with it); none for any other document. (23 Oct: pub, the card
/// lists them too: nary::outbound_files::card_items.)
pub async fn _linked_quotations(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &Entity,
) -> ApiResult<Vec<Quotation>> {
    if entity_type != "sales_document" {
        return Ok(vec![]);
    }
    let rows = crate::documents::core_::quotations_of(db, entity.id()).await?;
    Ok(rows
        .into_iter()
        .map(|(q, _)| q)
        .filter(|q| {
            !matches!(
                q.status.as_deref(),
                Some("superseded" | "rejected" | "expired")
            )
        })
        .collect())
}

/// The quotation as the adapter loads it (Python passes the same identity-mapped object). A deleted
/// quotation the adapter would not load is built from its row, as Python still holds it.
async fn quotation_entity(db: &mut PgConnection, q: &Quotation) -> ApiResult<Entity> {
    if let Some(e) = get_adapter("quotation")?
        .load(db, &q.id.to_string())
        .await?
    {
        return Ok(e);
    }
    let items: Vec<QuotationItem> =
        sqlx::query_as("SELECT * FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order")
            .bind(q.id)
            .fetch_all(&mut *db)
            .await?;
    let org = get_org(db, q.customer_org_id).await?;
    Ok(Entity::Quotation(Box::new(QuotationDoc {
        row: q.clone(),
        items,
        customer_org_name: org.map(|o| o.name),
    })))
}

/// The document's current verified copy, or a freshly issued one.
pub async fn _verified_pdf(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &Entity,
    user: &AuthUser,
) -> ApiResult<(Option<DocumentVerification>, Vec<u8>)> {
    if let Some(f) = seams::verified_pdf() {
        return f(db, entity_type.to_string(), entity.clone(), user.clone()).await;
    }
    _verified_pdf_real(db, entity_type, entity, user).await
}

async fn _verified_pdf_real(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &Entity,
    user: &AuthUser,
) -> ApiResult<(Option<DocumentVerification>, Vec<u8>)> {
    let rows: Vec<DocumentVerification> = sqlx::query_as(
        "SELECT * FROM document_verifications WHERE entity_type = $1 AND entity_id = $2 AND status = 'active' \
         ORDER BY issued_at DESC",
    )
    .bind(entity_type)
    .bind(entity.id())
    .fetch_all(&mut *db)
    .await?;
    let revision = get_adapter(entity_type)?.revision(entity);
    for mut v in rows {
        if v.pdf_object_key.as_deref().is_some_and(|k| !k.is_empty())
            && v.revision == revision
            && verification::evaluate(db, &mut v).await? == "valid"
        {
            // the sealed outbound copy (sealed now when it was issued before the seal or its seal failed)
            let (pdf, _) = verification::outbound_pdf(db, &mut v).await?;
            return Ok((Some(v), pdf));
        }
    }
    // A render failure comes back from issue() as a 502 "could not be rendered": nothing is sent.
    // issue() sealed it already; without a sealed copy (setting off, or the seal just failed and the Owner was
    // alerted) the master goes, without a second attempt in the same send
    let mut v = verification::issue(db, entity_type, &entity.id().to_string(), user).await?;
    let pdf = if v.sealed_object_key.is_some() {
        verification::outbound_pdf(db, &mut v).await?.0
    } else {
        verification::get_pdf(v.pdf_object_key.as_deref().unwrap_or_default())
            .await
            .map_err(ApiError::internal)?
    };
    Ok((Some(v), pdf))
}

/// "Quotation AR-Q/...-1.pdf" for a card. `Ok(Err(msg))` is Python's ValueError (the model can act on it).
pub async fn attachment_label(
    db: &mut PgConnection,
    item: &Value,
) -> ApiResult<Result<String, String>> {
    let etype = item
        .get("type")
        .and_then(|v| v.as_str())
        .filter(|t| ATTACHABLE.contains(t));
    let Some(etype) = etype else {
        return Ok(Err(format!(
            "Only {} can be attached",
            ATTACHABLE.join(", ")
        )));
    };
    let adapter = get_adapter(etype)?;
    let id = item.get("id");
    let id_s = if py_truthy(id) {
        py_str(id.unwrap())
    } else {
        String::new()
    };
    let Some(entity) = adapter.load(db, &id_s).await? else {
        return Ok(Err(format!(
            "No {} with id {}",
            pystr::lower(adapter.label),
            py_str(id.unwrap_or(&Value::Null))
        )));
    };
    Ok(Ok(format!(
        "{} {}",
        adapter.label,
        _filename(&adapter.number(&entity))
    )))
}

/// The verified PDFs of the sales documents a Nary email carries (a proposal brings its quotations).
/// The same rules as sending the document itself: the person's send permission, and the approval gate
/// (an unapproved one is submitted for approval and nothing is sent).
pub async fn email_attachments(
    db: &mut PgConnection,
    items: &[Value],
    user: &AuthUser,
) -> ApiResult<Vec<EmailAttachment>> {
    let mut out = vec![];
    // 23 Oct: each document once (as nary_documents)
    let mut seen: HashSet<(String, Uuid)> = HashSet::new();
    for item in items {
        let etype = item
            .get("type")
            .and_then(|v| v.as_str())
            .filter(|t| ATTACHABLE.contains(t));
        let Some(etype) = etype else {
            return Err(ApiError::http(
                422,
                format!("Only {} can be attached", ATTACHABLE.join(", ")),
            ));
        };
        let perm = sendable(etype).unwrap_or_default();
        if !user.is_superadmin && !user.perms.contains(perm) {
            return Err(ApiError::http(
                403,
                format!("Permission required to attach it: {perm}"),
            ));
        }
        let adapter = get_adapter(etype)?;
        let id = item.get("id");
        let id_s = if py_truthy(id) {
            py_str(id.unwrap())
        } else {
            String::new()
        };
        let Some(entity) = adapter.load(db, &id_s).await? else {
            return Err(ApiError::http(
                404,
                format!("The {} to attach is gone", pystr::lower(adapter.label)),
            ));
        };
        let status = entity.status().unwrap_or("").to_string();
        if verification::VOID_STATUSES.contains(&status.as_str())
            || matches!(status.as_str(), "superseded" | "rejected")
        {
            return Err(ApiError::http(
                400,
                format!(
                    "A {status} {} can't be attached",
                    pystr::lower(adapter.label)
                ),
            ));
        }
        let mut docs: Vec<(&str, Entity)> = vec![];
        let linked = _linked_quotations(db, etype, &entity).await?;
        docs.push((etype, entity));
        for q in &linked {
            docs.push(("quotation", quotation_entity(db, q).await?));
        }
        for (dtype, doc) in &docs {
            if !seen.insert((dtype.to_string(), doc.id())) {
                continue;
            }
            if let Err(e) = engine::ensure_or_submit(db, dtype, doc, user, "sent", None).await {
                if e.status().as_u16() == 409 && matches!(e, ApiError::Http { .. }) {
                    let a = get_adapter(dtype)?;
                    return Err(ApiError::http(
                        409,
                        format!(
                            "{} {} isn't approved yet; it went for approval. Nothing was sent: confirm the email \
                             again once it is approved.",
                            a.label,
                            a.number(doc)
                        ),
                    ));
                }
                return Err(e);
            }
            let (_, pdf) = _verified_pdf(db, dtype, doc, user).await?;
            out.push(EmailAttachment {
                filename: _filename(&get_adapter(dtype)?.number(doc)),
                content: pdf,
                content_type: "application/pdf".into(),
                mime_type: "application/pdf".into(),
            });
        }
    }
    Ok(out)
}

/// `_filename(number)`: alphanumerics and `-_.` kept, everything else a dash; dashes trimmed.
/// (Python's str.isalnum; Rust's is_alphanumeric differs only on rare combining marks.)
pub fn _filename(number: &str) -> String {
    let safe: String = number
        .chars()
        .map(|c| {
            if pystr::is_alnum(c) || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-');
    format!("{}.pdf", if safe.is_empty() { "document" } else { safe })
}

fn _email_html(
    entity_type: &str,
    entity: &Entity,
    number: &str,
    to_name: &str,
    message: Option<&str>,
    company: &crate::email_marketing::CompanyInfo,
    link: &str,
) -> ApiResult<(String, String)> {
    use crate::email_templates as t;
    let notes = pystr::strip(message.unwrap_or("")).to_string();
    let (cn, ce, cw) = (
        &company.company_trade,
        &company.company_email,
        &company.company_website,
    );
    let or_name = |d: &'static str| if to_name.is_empty() { d } else { to_name };
    if entity_type == "quotation" {
        let html =
            t::quotation_email_template(or_name("Customer"), number, cn, ce, cw, &notes, link);
        return Ok((format!("Quotation {number} {EM_DASH} {cn}"), html));
    }
    if entity_type == "invoice" {
        let due_date = match entity {
            Entity::Invoice(inv) => inv.due_date,
            _ => None,
        };
        let due = match due_date {
            Some(d) => render::fmt_date(d),
            None => "the agreed date".to_string(),
        };
        let html =
            t::invoice_email_template(or_name("Customer"), number, &due, cn, ce, cw, &notes, link);
        return Ok((format!("Invoice {number} {EM_DASH} {cn}"), html));
    }
    let label = get_adapter(entity_type)?.label;
    if entity_type == "sales_document" {
        let html = t::document_email_template(
            or_name("Customer"),
            label,
            number,
            cn,
            ce,
            cw,
            &notes,
            link,
        );
        let title = match entity {
            Entity::SalesDocument(d) => d.title.clone(),
            _ => String::new(),
        };
        return Ok((format!("{label} {number}: {title} {EM_DASH} {cn}"), html));
    }
    let html =
        t::document_email_template(or_name("Partner"), label, number, cn, ce, cw, &notes, link);
    Ok((format!("{label} {number} {EM_DASH} {cn}"), html))
}

/// What the conversation in the CRM inbox is linked to: (contact_id, org_id, deal_id); all None for
/// vendor documents (Python passes no keyword at all, the same defaults).
fn _crm_links(entity_type: &str, entity: &Entity) -> CrmLinks {
    match (entity_type, entity) {
        ("quotation", Entity::Quotation(q)) => CrmLinks {
            contact_id: q.row.customer_contact_id,
            org_id: q.row.customer_org_id,
            deal_id: q.row.deal_id,
        },
        ("invoice", Entity::Invoice(i)) => CrmLinks {
            contact_id: i.customer_contact_id,
            org_id: i.customer_org_id,
            deal_id: i.deal_id,
        },
        ("sales_document", Entity::SalesDocument(d)) => CrmLinks {
            contact_id: d.contact_id,
            org_id: d.organization_id,
            deal_id: d.deal_id,
        },
        _ => CrmLinks::default(),
    }
}

/// `links` of `_send_email`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrmLinks {
    pub contact_id: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub deal_id: Option<Uuid>,
}

/// The arguments of `_send_email(db, to, cc, subject, html, pdf, filename, *, user, links, extra=None)`.
#[derive(Debug, Clone)]
pub struct SendEmailArgs {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub html: String,
    pub pdf: Vec<u8>,
    pub filename: String,
    pub user: AuthUser,
    pub links: CrmLinks,
    pub extra: Option<Vec<(String, Vec<u8>)>>,
    /// R&D 26 Oct: blind copies (envelope only) and the CRM inbox it goes from (None: the deal's, else hello@)
    pub bcc: Vec<String>,
    pub from_inbox: Option<String>,
}

tokio::task_local! {
    /// R&D 26 Oct: the From inbox and Bcc the person chose for this send (the Send dialog).
    static MAIL_OPTIONS: (Option<String>, Vec<String>);
}

/// Runs `fut` (a [`send`]) with the From inbox and Bcc chosen for it.
pub async fn with_mail_options<F: std::future::Future>(
    from_inbox: Option<String>,
    bcc: Vec<String>,
    fut: F,
) -> F::Output {
    MAIL_OPTIONS.scope((from_inbox, bcc), fut).await
}

fn mail_options() -> (Option<String>, Vec<String>) {
    MAIL_OPTIONS.try_with(|o| o.clone()).unwrap_or_default()
}

/// Email from the CRM mailbox, then keep it where people look for it: a conversation in the CRM
/// inbox (so the customer's reply threads onto it) and a copy in the mailbox's Sent folder.
pub async fn _send_email(db: &mut PgConnection, args: SendEmailArgs) -> ApiResult<String> {
    if let Some(f) = seams::send_email() {
        return f(db, args).await;
    }
    _send_email_real(db, args).await
}

async fn _send_email_real(db: &mut PgConnection, args: SendEmailArgs) -> ApiResult<String> {
    mail::send_document_email(db, args).await
}

/// `routers/whatsapp.py` `_WA_PASSTHROUGH_STATUS`.
const WA_PASSTHROUGH_STATUS: [u16; 5] = [400, 404, 410, 413, 503];

/// `routers/whatsapp.py` `_raise_wa_error(resp)`.
fn _raise_wa_error(resp: &crate::whatsapp::HttpxResponse) -> ApiError {
    let text = String::from_utf8_lossy(&resp.content).to_string();
    let detail: Value = match resp.json() {
        Ok(Value::Object(payload)) => {
            let mut detail = match payload.get("error") {
                Some(e) if crate::whatsapp::py_truthy(e) => e.clone(),
                _ => Value::String(text.clone()),
            };
            if let Some(hint) = payload
                .get("status")
                .filter(|h| crate::whatsapp::py_truthy(h))
            {
                detail = Value::String(format!(
                    "{} (WhatsApp client status: {})",
                    py_str(&detail),
                    py_str(hint)
                ));
            }
            detail
        }
        // not JSON, or not a dict (`.get` raises AttributeError): the except branch
        _ => Value::String(if text.is_empty() {
            "no response body".to_string()
        } else {
            text
        }),
    };
    tracing::warn!(
        "WhatsApp service {} from {}: {}",
        resp.status,
        resp.url,
        py_str(&detail)
    );
    let status = if WA_PASSTHROUGH_STATUS.contains(&resp.status) {
        resp.status
    } else {
        502
    };
    ApiError::http_json(status, detail)
}

async fn _send_whatsapp(to: &str, pdf: &[u8], filename: &str, caption: &str) -> ApiResult<Value> {
    use crate::whatsapp::{httpx_json_body, httpx_url, HttpxError, HttpxResponse};
    let base = granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone();
    if base.is_empty() {
        return Err(ApiError::http(503, "WhatsApp service not configured"));
    }
    let chat = if to.contains('@') {
        to.to_string()
    } else {
        granary_compat::text::utils::normalize_phone(Some(to))
    };
    if chat.is_empty() {
        return Err(ApiError::http(
            422,
            "Enter a WhatsApp number or pick a chat",
        ));
    }
    let path = format!("{base}/send-media");
    let body = json!({"to": chat, "data": base64::engine::general_purpose::STANDARD.encode(pdf),
                      "mimetype": "application/pdf", "filename": filename, "caption": caption});
    // _wa_request: transport failures become clean HTTP errors.
    let attempt: Result<HttpxResponse, HttpxError> = async {
        let url = httpx_url(&path, &[])?;
        let mut rb = crate::http::client()
            .post(url)
            .timeout(Duration::from_secs(120));
        for (k, v) in crate::whatsapp::_headers() {
            rb = rb.header(k, v);
        }
        HttpxResponse::read(rb.body(httpx_json_body(&body)).send().await?).await
    }
    .await;
    let resp = match attempt {
        Ok(r) => r,
        Err(HttpxError::Timeout(e)) => {
            tracing::warn!("WhatsApp service timeout on {}: {}", path, e);
            return Err(ApiError::http(
                504,
                "WhatsApp service did not respond in time. It may be busy or restarting.",
            ));
        }
        Err(e) => {
            tracing::warn!("WhatsApp service unreachable on {}: {}", path, e);
            return Err(ApiError::http(
                502,
                "WhatsApp service is unreachable; it may be restarting. Try again shortly."
                    .to_string(),
            ));
        }
    };
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    resp.json().map_err(ApiError::from)
}

/// `_mark_sent(entity_type, entity)` for a quotation row.
async fn mark_sent_quotation(db: &mut PgConnection, q: &mut Quotation) -> ApiResult<()> {
    if matches!(q.status.as_deref(), Some("draft" | "sent")) {
        if q.full_number.as_deref().unwrap_or("").is_empty() {
            let group = nonempty(q.revision_group.clone()).unwrap_or_else(|| q.number.clone());
            // f"{...}-{entity.revision}": a NULL revision prints "None", as Python does.
            let rev = q
                .revision
                .map(|r| r.to_string())
                .unwrap_or_else(|| "None".into());
            q.full_number = Some(format!("{group}-{rev}"));
        }
        q.status = Some("sent".into());
        let row: (DateTime<Utc>,) = sqlx::query_as(
            "UPDATE quotations SET full_number = $2, status = $3, updated_at = now() WHERE id = $1 RETURNING updated_at",
        )
        .bind(q.id)
        .bind(&q.full_number)
        .bind(&q.status)
        .fetch_one(&mut *db)
        .await?;
        q.updated_at = row.0;
    }
    Ok(())
}

async fn _mark_sent(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &mut Entity,
) -> ApiResult<()> {
    match (entity_type, entity) {
        ("quotation", Entity::Quotation(q)) => mark_sent_quotation(db, &mut q.row).await?,
        ("invoice", Entity::Invoice(inv)) => {
            if inv.status.as_deref() == Some("draft") {
                inv.status = Some("sent".into());
                sqlx::query(
                    "UPDATE invoices SET status = 'sent', updated_at = now() WHERE id = $1",
                )
                .bind(inv.id)
                .execute(&mut *db)
                .await?;
            }
        }
        ("purchase_order", Entity::PurchaseOrder(po)) => {
            if po.row.status.as_deref() == Some("draft") {
                po.row.status = Some("sent".into());
                sqlx::query(
                    "UPDATE purchase_orders SET status = 'sent', updated_at = now() WHERE id = $1",
                )
                .bind(po.row.id)
                .execute(&mut *db)
                .await?;
            }
        }
        ("sales_document", Entity::SalesDocument(d)) => {
            if matches!(d.status.as_str(), "approved" | "sent") {
                d.status = "sent".into();
                d.sent_at = Some(now());
                sqlx::query("UPDATE sales_documents SET status = 'sent', sent_at = $2, updated_at = now() WHERE id = $1")
                    .bind(d.id)
                    .bind(d.sent_at)
                    .execute(&mut *db)
                    .await?;
                crate::nary::proposal_lifecycle::close_review(
                    db,
                    d.id,
                    crate::nary::wa_send::current_card(),
                    "the proposal was sent to the customer.",
                    false, // #22c: its quotation card stays
                )
                .await?;
            }
        }
        // A work order's status tracks the work itself (activation is separate): the send is only recorded.
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn _log_activity(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &Entity,
    user: &AuthUser,
    channel: &str,
    to: &str,
    number: &str,
    subject: &str,
    message_id: Option<&str>,
) -> ApiResult<()> {
    let (target_type, target_id, contact_id, deal_id) =
        if matches!(entity_type, "quotation" | "invoice" | "sales_document") {
            let (cid, _) = _customer_ids(entity_type, entity);
            let deal = deal_id_of(entity);
            match deal {
                Some(d) => ("deal", Some(d), cid, deal),
                None => ("contact", cid, cid, deal),
            }
        } else {
            ("vendor", vendor_id_of(entity), None, None)
        };
    let Some(target_id) = target_id else {
        return Ok(());
    };
    let label = get_adapter(entity_type)?.label;
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, contact_id, deal_id, activity_type, direction, subject, \
         content, is_completed, completed_at, channel_message_id, channel_data, created_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, 'outbound', $7, $8, true, $9, $10, $11, $12)",
    )
    .bind(Uuid::new_v4())
    .bind(target_type)
    .bind(target_id)
    .bind(contact_id)
    .bind(deal_id)
    .bind(channel)
    .bind(subject)
    .bind(format!("{label} {number} sent to {to} ({channel}) with its verified PDF"))
    .bind(now())
    .bind(message_id)
    .bind(json!({"document_type": entity_type, "document_id": entity.id().to_string()}))
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

// --- Accidental-resend guard (Redis; fails open) ------------------------------------------------------

/// Just the Redis calls the guard makes (`set(key, value, nx=, ex=)`, `get`, `delete`).
#[async_trait::async_trait]
pub trait GuardRedis: Send + Sync {
    /// Returns false when `nx` and the key exists (redis-py returns None).
    async fn set(&self, key: &str, value: &str, nx: bool, ex: u64) -> anyhow::Result<bool>;
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    async fn delete(&self, key: &str) -> anyhow::Result<()>;
}

/// The real client: `granary_core::redis_client::get_redis()` (Python built its own from REDIS_URL).
struct RealRedis;

#[async_trait::async_trait]
impl GuardRedis for RealRedis {
    async fn set(&self, key: &str, value: &str, nx: bool, ex: u64) -> anyhow::Result<bool> {
        let mut c = granary_core::redis_client::get_redis().conn().await?;
        let mut cmd = redis::cmd("SET");
        cmd.arg(key).arg(value);
        if nx {
            cmd.arg("NX");
        }
        cmd.arg("EX").arg(ex);
        let r: Option<String> = cmd.query_async(&mut c).await?;
        Ok(r.is_some())
    }
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let mut c = granary_core::redis_client::get_redis().conn().await?;
        Ok(redis::cmd("GET").arg(key).query_async(&mut c).await?)
    }
    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let mut c = granary_core::redis_client::get_redis().conn().await?;
        let _: i64 = redis::cmd("DEL").arg(key).query_async(&mut c).await?;
        Ok(())
    }
}

/// `_redis()`.
pub async fn _redis() -> anyhow::Result<Arc<dyn GuardRedis>> {
    if let Some(f) = seams::redis() {
        return f().await;
    }
    Ok(Arc::new(RealRedis))
}

pub fn _send_key(entity_type: &str, entity: &Entity, channel: &str, to: &[String]) -> String {
    send_key_for_id(entity_type, &entity.id().to_string(), channel, to)
}

/// `_send_key` given `str(entity.id)`.
pub fn send_key_for_id(entity_type: &str, entity_id: &str, channel: &str, to: &[String]) -> String {
    // sorted(t.lower() for t in to): Python's str.lower, then hashlib.sha1(...)[:16]
    let lowered: Vec<String> = to.iter().map(|t| pystr::lower(t)).collect();
    let lowered: Vec<&str> = lowered.iter().map(|s| s.as_str()).collect();
    granary_compat::crypto::hashes::doc_send_key(entity_type, entity_id, channel, &lowered)
}

/// Refuse an identical send that is still running, or (unless forced) one that just went out.
/// Returns whether the in-flight lock is held (so it must be released). Redis down: no guard.
async fn _guard(key: &str, force: bool, label: &str, to: &[String]) -> ApiResult<bool> {
    let attempt: Result<bool, Result<ApiError, anyhow::Error>> = async {
        let r = _redis().await.map_err(Err)?;
        if !r.set(&format!("{key}:inflight"), "1", true, INFLIGHT_SECONDS).await.map_err(Err)? {
            return Err(Ok(ApiError::http_json(
                409,
                json!({
                    "code": "send_in_progress",
                    "message": format!("This {} is already being sent to {}. Wait a moment and check before trying again.",
                                       pystr::lower(label), to.join(", ")),
                }),
            )));
        }
        if !force {
            let sent_at = r.get(&format!("{key}:sent")).await.map_err(Err)?;
            if let Some(sent_at) = sent_at.filter(|s| !s.is_empty()) {
                r.delete(&format!("{key}:inflight")).await.map_err(Err)?;
                return Err(Ok(ApiError::http_json(
                    409,
                    json!({
                        "code": "recently_sent", "sent_at": sent_at, "to": to,
                        "message": format!("This {} was already sent to {} a few minutes ago.",
                                           pystr::lower(label), to.join(", ")),
                    }),
                )));
            }
        }
        Ok(true)
    }
    .await;
    match attempt {
        Ok(v) => Ok(v),
        Err(Ok(http)) => Err(http),
        Err(Err(exc)) => {
            tracing::warn!(error = %truncate(&exc.to_string(), 200), "document_send_guard_unavailable");
            Ok(false)
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `datetime.now(timezone.utc).isoformat()`.
fn iso_now() -> String {
    granary_compat::pyjson::PyDateTime::from_chrono_utc(&now()).isoformat()
}

async fn _remember_sent(key: &str) {
    let attempt = async {
        let r = _redis().await?;
        r.set(
            &format!("{key}:sent"),
            &iso_now(),
            false,
            RECENT_SEND_SECONDS,
        )
        .await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(exc) = attempt {
        tracing::warn!(error = %truncate(&exc.to_string(), 200), "document_send_guard_unavailable");
    }
}

async fn _release(key: &str) {
    if let Ok(r) = _redis().await {
        let _ = r.delete(&format!("{key}:inflight")).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn send(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &mut Entity,
    user: &AuthUser,
    channel: &str,
    to: &[String],
    cc: Option<&[String]>,
    message: Option<&str>,
    to_name: Option<&str>,
    force: bool,
) -> ApiResult<Value> {
    let adapter = adapter_for(entity_type)?;
    if !CHANNELS.contains(&channel) {
        return Err(ApiError::http(422, "channel must be email or whatsapp"));
    }
    let to: Vec<String> = to
        .iter()
        .map(|t| pystr::strip(t).to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let cc: Vec<String> = cc
        .unwrap_or(&[])
        .iter()
        .map(|c| pystr::strip(c).to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if to.is_empty() {
        return Err(ApiError::http(422, "Enter at least one recipient"));
    }
    if channel == "whatsapp" && to.len() != 1 {
        return Err(ApiError::http(
            422,
            "Send a WhatsApp message to one chat at a time",
        ));
    }
    let status = entity.status().unwrap_or("").to_string();
    if verification::VOID_STATUSES.contains(&status.as_str())
        || matches!(status.as_str(), "superseded" | "accepted" | "rejected")
    {
        return Err(ApiError::http(
            400,
            format!("A {status} {} cannot be sent", pystr::lower(adapter.label)),
        ));
    }

    // The gate: not approved -> submitted (or still waiting) and nothing is delivered. A proposal goes
    // with its quotations, so each of them must be approved too.
    engine::ensure_or_submit(db, entity_type, &*entity, user, "sent", None).await?;
    for q in _linked_quotations(db, entity_type, entity).await? {
        let qe = quotation_entity(db, &q).await?;
        engine::ensure_or_submit(db, "quotation", &qe, user, "sent with the proposal", None)
            .await?;
    }
    if let Entity::Invoice(inv) = &*entity {
        if entity_type == "invoice"
            && inv.invoice_type.as_deref() == Some("credit_note")
            && inv.status.as_deref() == Some("draft")
        {
            // Refuse before anything reaches the customer: the note can't take effect on a cancelled invoice.
            let original: Option<Invoice> = match inv.credit_of_invoice_id {
                Some(id) => {
                    sqlx::query_as("SELECT * FROM invoices WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&mut *db)
                        .await?
                }
                None => None,
            };
            if original
                .as_ref()
                .is_none_or(|o| o.is_deleted || o.status.as_deref() == Some("cancelled"))
            {
                return Err(ApiError::http(
                    400,
                    "The invoice this credit note reverses is already cancelled or gone; this credit note can't be sent.",
                ));
            }
        }
    }

    let key = _send_key(entity_type, entity, channel, &to);
    let locked = _guard(&key, force, adapter.label, &to).await?;
    let result = _deliver(
        db,
        entity_type,
        entity,
        user,
        channel,
        &to,
        &cc,
        message,
        to_name,
        adapter,
        &key,
    )
    .await;
    if locked {
        _release(&key).await;
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn _deliver(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &mut Entity,
    user: &AuthUser,
    channel: &str,
    to: &[String],
    cc: &[String],
    message: Option<&str>,
    to_name: Option<&str>,
    adapter: &'static DocumentAdapter,
    key: &str,
) -> ApiResult<Value> {
    let (issued, pdf) = _verified_pdf(db, entity_type, entity, user).await?;
    let issued = issued.ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "'NoneType' object has no attribute 'token'"
        ))
    })?;
    let number = adapter.number(entity);
    let filename = _filename(&number);
    let mut quotes = _linked_quotations(db, entity_type, entity).await?;
    let mut extra: Vec<(String, Vec<u8>)> = vec![]; // each quotation with its own verified PDF and QR
    for q in &quotes {
        let qe = quotation_entity(db, q).await?;
        let (_, qpdf) = _verified_pdf(db, "quotation", &qe, user).await?;
        let qnum = nonempty(q.full_number.clone()).unwrap_or_else(|| q.number.clone());
        extra.push((_filename(&qnum), qpdf));
    }
    let link = verification::verify_url(issued.token);
    let company = _company(db).await?;

    let message_id: Option<String>;
    let subject: String;
    if channel == "email" {
        let (s, html) = _email_html(
            entity_type,
            entity,
            &number,
            to_name.unwrap_or(""),
            message,
            &company,
            &link,
        )?;
        subject = s;
        message_id = Some(
            _send_email(
                db,
                SendEmailArgs {
                    to: to.to_vec(),
                    cc: cc.to_vec(),
                    subject: subject.clone(),
                    html,
                    pdf,
                    filename,
                    user: user.clone(),
                    links: _crm_links(entity_type, entity),
                    extra: if extra.is_empty() {
                        None
                    } else {
                        Some(extra.clone())
                    },
                    from_inbox: mail_options().0,
                    bcc: mail_options().1,
                },
            )
            .await?,
        );
    } else {
        subject = format!("{} {number}", adapter.label);
        let msg = pystr::strip(message.unwrap_or("")).to_string();
        let parts = [
            msg,
            format!("{} {number} from {}.", adapter.label, company.company_trade),
            format!("Verify this document: {link}"),
        ];
        let caption = parts
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");
        let result = _send_whatsapp(&to[0], &pdf, &filename, &caption).await?;
        // result.get("messageId"): None when absent; Python stores whatever value it is.
        message_id = match result.get("messageId") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(v) => Some(py_str(v)),
        };
        for ((qfile, qpdf), q) in extra.iter().zip(quotes.iter()) {
            // then each quotation, in order
            let qnum = nonempty(q.full_number.clone()).unwrap_or_else(|| q.number.clone());
            _send_whatsapp(&to[0], qpdf, qfile, &format!("Quotation {qnum}")).await?;
        }
    }

    _remember_sent(key).await;
    let was_draft = entity.status() == Some("draft");
    _mark_sent(db, entity_type, entity).await?;
    if entity_type == "invoice" && was_draft && entity.status() == Some("sent") {
        // posts a commercial invoice; a credit note cancels its original
        if let Entity::Invoice(inv) = entity {
            crate::invoicing::on_invoice_issued(db, inv, Some(user.id)).await?;
        }
    }
    if entity_type == "quotation" && was_draft && entity.status() == Some("sent") {
        // The first send starts Nary's follow-up cadence, like the status change does.
        // (The send itself is already on the timeline: _log_activity below.)
        if let Entity::Quotation(q) = &*entity {
            let mut extra_payload = Map::new();
            extra_payload.insert("channel".into(), json!(channel));
            _emit_quotation(db, "quotation.sent", &q.row, user, extra_payload).await?;
        }
    }
    for q in quotes.iter_mut() {
        // sent with the proposal: each starts its own follow-up cadence
        if matches!(q.status.as_deref(), Some("draft" | "approved")) {
            // Python quirk kept: an "approved" quotation is not changed by _mark_sent (only draft/sent are),
            // yet its quotation.sent event is still emitted.
            mark_sent_quotation(db, q).await?;
            let mut extra_payload = Map::new();
            extra_payload.insert("channel".into(), json!(channel));
            extra_payload.insert("with_proposal".into(), json!(entity.id().to_string()));
            _emit_quotation(db, "quotation.sent", q, user, extra_payload).await?;
        }
    }
    _log_activity(
        db,
        entity_type,
        entity,
        user,
        channel,
        &to.join(", "),
        &number,
        &subject,
        message_id.as_deref(),
    )
    .await?;
    tracing::info!(entity_type, entity_id = %entity.id(), channel, recipients = to.len() + cc.len(),
                   verification_id = %issued.id, by = %user.id, "document_sent");
    Ok(
        json!({"id": entity.id().to_string(), "status": entity.status(), "channel": channel, "to": to,
              "verification_id": issued.id.to_string(), "verify_url": link}),
    )
}

/// `routers/sales.py` `_emit_quotation(db, type_, quotation, user, **extra)` (a router helper whose body is
/// one `nary.events.emit`, called directly here). Tests replace it through `seams`.
pub async fn _emit_quotation(
    db: &mut PgConnection,
    type_: &str,
    q: &Quotation,
    user: &AuthUser,
    extra: Map<String, Value>,
) -> ApiResult<()> {
    if let Some(f) = seams::emit_quotation() {
        return f(db, type_.to_string(), q.clone(), user.clone(), extra).await;
    }
    let mut payload = Map::new();
    payload.insert(
        "number".into(),
        json!(nonempty(q.full_number.clone()).unwrap_or_else(|| q.number.clone())),
    );
    payload.insert("deal_id".into(), json!(q.deal_id.map(|d| d.to_string())));
    payload.insert(
        "customer_org_id".into(),
        json!(q.customer_org_id.map(|d| d.to_string())),
    );
    payload.insert(
        "total".into(),
        json!(q
            .total
            .map(|t| granary_compat::pyjson::str_decimal(&t.into()))),
    );
    payload.insert("currency".into(), json!(q.currency));
    for (k, v) in extra {
        payload.insert(k, v);
    }
    crate::nary::events::emit(
        db,
        type_,
        Some("quotation"),
        Some(q.id),
        Some(Value::Object(payload)),
        Some(user.id),
    )
    .await?;
    Ok(())
}

async fn _company(db: &mut PgConnection) -> ApiResult<crate::email_marketing::CompanyInfo> {
    Ok(crate::email_marketing::get_company_info(db).await?)
}

/// The CRM-mailbox side of `_send_email` (email.py `load_crm_smtp` / `_send_via_config`, crm_inbox.py
/// `record_outbound` / `save_to_sent`).
mod mail {
    use super::*;

    pub async fn send_document_email(
        db: &mut PgConnection,
        args: SendEmailArgs,
    ) -> ApiResult<String> {
        use crate::email::{Attachment, Capture};
        // R&D 26 Oct: from the inbox chosen, else the deal's inbox, else hello@
        let mailbox =
            crate::crm_mailboxes::pick(db, args.from_inbox.as_deref(), None, args.links.deal_id)
                .await
                .map_err(|e| match e {
                    crate::email::EmailError::Value(m) => ApiError::http(400, m),
                    other => ApiError::internal(other),
                })?;
        let cfg = crate::crm_mailboxes::smtp(db, &mailbox)
            .await
            .map_err(ApiError::internal)?;
        if cfg.host.is_empty() || cfg.from_email.is_empty() {
            return Err(ApiError::http(
                400,
                format!(
                    "The CRM inbox {} is not configured (Settings \u{2192} Email & WhatsApp \u{2192} CRM inboxes)",
                    mailbox.address
                ),
            ));
        }
        let domain = cfg
            .from_email
            .rsplit('@')
            .next()
            .filter(|d| !d.is_empty())
            .unwrap_or("granary.local");
        let message_id = format!("<{}@{domain}>", Uuid::new_v4());
        let pdf_att = |filename: &str, content: &[u8]| Attachment {
            filename: filename.to_string(),
            content: content.to_vec(),
            mime_type: Some("application/pdf".into()),
        };
        let mut attachments = vec![pdf_att(&args.filename, &args.pdf)];
        for (f, data) in args.extra.as_deref().unwrap_or(&[]) {
            attachments.push(pdf_att(f, data));
        }
        let mut sent = Capture::default();
        let headers = vec![("Message-ID".to_string(), message_id.clone())];
        let cc = if args.cc.is_empty() {
            None
        } else {
            Some(args.cc.as_slice())
        }; // cc or None
        if let Err(exc) = crate::email::_send_via_config_bcc(
            &cfg,
            &args.to,
            &args.subject,
            &args.html,
            cc,
            &args.bcc,
            Some(&attachments),
            Some(&mut *db),
            Some(&headers),
            Some(&mut sent),
        )
        .await
        {
            return Err(ApiError::http(
                502,
                format!("The email could not be sent: {exc}"),
            ));
        }

        // The email is out; recording it must never turn the send into an error.
        if let Err(exc) = crate::crm_inbox::record_outbound(
            db,
            &cfg,
            &message_id,
            &args.to,
            Some(&args.cc),
            &args.subject,
            &args.html,
            Some(&attachments),
            Some(args.user.id),
            args.links.contact_id,
            args.links.org_id,
            args.links.deal_id,
        )
        .await
        {
            tracing::warn!(error = %truncate(&exc.to_string(), 200), "document_email_not_recorded");
        }
        if !args.bcc.is_empty() {
            let _ = sqlx::query(
                "UPDATE crm_email_messages SET bcc_emails = $2 WHERE message_id_header = $1",
            )
            .bind(&message_id)
            .bind(&args.bcc)
            .execute(&mut *db)
            .await;
        }
        for to in &args.to {
            crate::nary::outbound_log::record(
                db,
                crate::nary::outbound_log::Outbound {
                    channel: "email",
                    email: Some(to),
                    mailbox_id: Some(mailbox.id),
                    contact_id: args.links.contact_id,
                    deal_id: args.links.deal_id,
                    kind: "document",
                    message_ref: Some(&message_id),
                    ..Default::default()
                },
            )
            .await;
        }
        if let Some(raw) = sent.raw.filter(|r| !r.is_empty()) {
            let copy = async {
                match seams::save_to_sent() {
                    Some(f) => f(raw).await,
                    None => crate::crm_inbox::save_to_sent_in(&mailbox.prefix, &raw).await,
                }
            };
            // incl. timeout; the next poll can't recover it, but the mail went
            if tokio::time::timeout(Duration::from_secs(20), copy)
                .await
                .is_err()
            {
                tracing::warn!(error = "", "document_email_sent_copy_failed");
            }
        }
        Ok(message_id)
    }
}

/// Test seams for what Python's tests monkeypatch on this module.
pub mod seams {
    use super::*;

    pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
    pub type RedisFn =
        Arc<dyn Fn() -> BoxFut<'static, anyhow::Result<Arc<dyn GuardRedis>>> + Send + Sync>;
    pub type SendEmailFn = Arc<
        dyn for<'a> Fn(&'a mut PgConnection, SendEmailArgs) -> BoxFut<'a, ApiResult<String>>
            + Send
            + Sync,
    >;
    pub type VerifiedPdfFn = Arc<
        dyn for<'a> Fn(
                &'a mut PgConnection,
                String,
                Entity,
                AuthUser,
            )
                -> BoxFut<'a, ApiResult<(Option<DocumentVerification>, Vec<u8>)>>
            + Send
            + Sync,
    >;
    pub type EmitQuotationFn = Arc<
        dyn for<'a> Fn(
                &'a mut PgConnection,
                String,
                Quotation,
                AuthUser,
                Map<String, Value>,
            ) -> BoxFut<'a, ApiResult<()>>
            + Send
            + Sync,
    >;

    static REDIS: RwLock<Option<RedisFn>> = RwLock::new(None);
    static SEND_EMAIL: RwLock<Option<SendEmailFn>> = RwLock::new(None);
    static VERIFIED_PDF: RwLock<Option<VerifiedPdfFn>> = RwLock::new(None);
    static EMIT_QUOTATION: RwLock<Option<EmitQuotationFn>> = RwLock::new(None);
    /// crm_inbox.save_to_sent (Python's tests patch it on crm_inbox; it has no seam there).
    pub type SaveToSentFn = Arc<dyn Fn(Vec<u8>) -> BoxFut<'static, ()> + Send + Sync>;
    static SAVE_TO_SENT: RwLock<Option<SaveToSentFn>> = RwLock::new(None);
    pub fn set_save_to_sent(f: Option<SaveToSentFn>) {
        *SAVE_TO_SENT.write().unwrap() = f;
    }
    pub(super) fn save_to_sent() -> Option<SaveToSentFn> {
        SAVE_TO_SENT.read().unwrap().clone()
    }

    pub fn set_redis(f: Option<RedisFn>) {
        *REDIS.write().unwrap() = f;
    }
    pub fn set_send_email(f: Option<SendEmailFn>) {
        *SEND_EMAIL.write().unwrap() = f;
    }
    pub fn set_verified_pdf(f: Option<VerifiedPdfFn>) {
        *VERIFIED_PDF.write().unwrap() = f;
    }
    pub fn set_emit_quotation(f: Option<EmitQuotationFn>) {
        *EMIT_QUOTATION.write().unwrap() = f;
    }
    /// Remove every override.
    pub fn reset() {
        set_redis(None);
        set_send_email(None);
        set_verified_pdf(None);
        set_emit_quotation(None);
        set_save_to_sent(None);
    }

    pub(super) fn redis() -> Option<RedisFn> {
        REDIS.read().unwrap().clone()
    }
    pub(super) fn send_email() -> Option<SendEmailFn> {
        SEND_EMAIL.read().unwrap().clone()
    }
    pub(super) fn verified_pdf() -> Option<VerifiedPdfFn> {
        VERIFIED_PDF.read().unwrap().clone()
    }
    pub(super) fn emit_quotation() -> Option<EmitQuotationFn> {
        EMIT_QUOTATION.read().unwrap().clone()
    }
}

// batch 4 D2: Nary sends sales documents on WhatsApp as well as by email, and marks them sent like the
// ERP's Send does (status, invoice posting, the quotation.sent event, the timeline and the resend guard).

/// One sales document a Nary message carries, with its verified PDF (a proposal's quotations come as
/// their own items, `with_proposal` naming the proposal).
#[derive(Debug, Clone)]
pub struct NaryDoc {
    pub entity_type: String,
    pub entity: Entity,
    pub label: &'static str,
    pub number: String,
    pub filename: String,
    pub pdf: Vec<u8>,
    pub verify_url: Option<String>,
    pub with_proposal: Option<Uuid>,
}

/// The verified PDFs of the documents a Nary message carries: the same rules as `email_attachments` (the
/// sender's send permission, refused statuses, and the approval gate: an unapproved one is submitted for
/// approval and nothing is sent), keeping each document and its verify link for the caption and the
/// "sent" marking.
pub async fn nary_documents(
    db: &mut PgConnection,
    items: &[Value],
    user: &AuthUser,
) -> ApiResult<Vec<NaryDoc>> {
    let mut out = vec![];
    // 23 Oct: each document once, whether it was given on its own or came with a proposal (one email
    // carried its quotation twice)
    let mut seen: HashSet<(String, Uuid)> = HashSet::new();
    for item in items {
        let etype = item
            .get("type")
            .and_then(|v| v.as_str())
            .filter(|t| ATTACHABLE.contains(t));
        let Some(etype) = etype else {
            return Err(ApiError::http(
                422,
                format!("Only {} can be attached", ATTACHABLE.join(", ")),
            ));
        };
        let perm = sendable(etype).unwrap_or_default();
        if !user.is_superadmin && !user.perms.contains(perm) {
            return Err(ApiError::http(
                403,
                format!("Permission required to attach it: {perm}"),
            ));
        }
        let adapter = get_adapter(etype)?;
        let id_s = item.get("id").map(py_str).unwrap_or_default();
        let Some(entity) = adapter.load(db, &id_s).await? else {
            return Err(ApiError::http(
                404,
                format!("The {} to attach is gone", pystr::lower(adapter.label)),
            ));
        };
        let status = entity.status().unwrap_or("").to_string();
        if verification::VOID_STATUSES.contains(&status.as_str())
            || matches!(status.as_str(), "superseded" | "rejected")
        {
            return Err(ApiError::http(
                400,
                format!(
                    "A {status} {} can't be attached",
                    pystr::lower(adapter.label)
                ),
            ));
        }
        let proposal_id = entity.id();
        let linked = _linked_quotations(db, etype, &entity).await?;
        let mut docs: Vec<(&str, Entity, Option<Uuid>)> = vec![(etype, entity, None)];
        for q in &linked {
            docs.push((
                "quotation",
                quotation_entity(db, q).await?,
                Some(proposal_id),
            ));
        }
        for (dtype, doc, with) in docs {
            if !seen.insert((dtype.to_string(), doc.id())) {
                continue;
            }
            let a = get_adapter(dtype)?;
            if let Err(e) = engine::ensure_or_submit(db, dtype, &doc, user, "sent", None).await {
                if e.status().as_u16() == 409 && matches!(e, ApiError::Http { .. }) {
                    return Err(ApiError::http(
                        409,
                        format!(
                            "{} {} isn't approved yet; it went for approval. Nothing was sent: confirm the \
                             message again once it is approved.",
                            a.label,
                            a.number(&doc)
                        ),
                    ));
                }
                return Err(e);
            }
            let (issued, pdf) = _verified_pdf(db, dtype, &doc, user).await?;
            let number = a.number(&doc);
            out.push(NaryDoc {
                entity_type: dtype.to_string(),
                label: a.label,
                filename: _filename(&number),
                number,
                pdf,
                verify_url: issued.map(|v| verification::verify_url(v.token)),
                entity: doc,
                with_proposal: with,
            });
        }
    }
    Ok(out)
}

/// After a Nary message with documents went out: each is marked sent as the ERP's Send marks it (a draft
/// invoice is issued and posted, a quotation's first send starts its follow-up cadence, a proposal's
/// quotations are marked with it), the send is on the timeline, and the resend guard knows it.
pub async fn nary_mark_sent(
    db: &mut PgConnection,
    docs: &mut [NaryDoc],
    user: &AuthUser,
    channel: &str,
    to: &str,
    message_id: Option<&str>,
) -> ApiResult<()> {
    for d in docs.iter_mut() {
        _remember_sent(&_send_key(
            &d.entity_type,
            &d.entity,
            channel,
            &[to.to_string()],
        ))
        .await;
        if let Some(proposal) = d.with_proposal {
            if let Entity::Quotation(q) = &mut d.entity {
                if matches!(q.row.status.as_deref(), Some("draft" | "approved")) {
                    mark_sent_quotation(db, &mut q.row).await?;
                    let mut extra = Map::new();
                    extra.insert("channel".into(), json!(channel));
                    extra.insert("with_proposal".into(), json!(proposal.to_string()));
                    _emit_quotation(db, "quotation.sent", &q.row, user, extra).await?;
                }
            }
            continue;
        }
        let was_draft = d.entity.status() == Some("draft");
        _mark_sent(db, &d.entity_type, &mut d.entity).await?;
        if d.entity_type == "invoice" && was_draft && d.entity.status() == Some("sent") {
            if let Entity::Invoice(inv) = &mut d.entity {
                crate::invoicing::on_invoice_issued(db, inv, Some(user.id)).await?;
            }
        }
        if d.entity_type == "quotation" && was_draft && d.entity.status() == Some("sent") {
            if let Entity::Quotation(q) = &d.entity {
                let mut extra = Map::new();
                extra.insert("channel".into(), json!(channel));
                _emit_quotation(db, "quotation.sent", &q.row, user, extra).await?;
            }
        }
        let subject = format!("{} {}", d.label, d.number);
        _log_activity(
            db,
            &d.entity_type,
            &d.entity,
            user,
            channel,
            to,
            &d.number,
            &subject,
            message_id,
        )
        .await?;
    }
    Ok(())
}
