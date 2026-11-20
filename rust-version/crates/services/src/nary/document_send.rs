//! doclib A3 (21 Oct 2026, Rust only): Nary sends documents of the library (Knowledge Base -> Documents) to
//! customers.
//!
//! ```text
//!     propose_document_send   a card for the action document.send (Nary -> Autonomy, Ask by default, never
//!                             promoted): the document (given, or picked from the deal's products in the
//!                             customer's language, else English), its live version label, its language and
//!                             the watermark line the customer's copy will carry, plus the covering text
//!     document.send           on confirm: the covering text and the stamped copy (doc_library::deliver::
//!                             customer_copy, logged in library_downloads as nary_send) through the existing
//!                             WhatsApp document / email paths of batch 4 (channel_actions::send_reply)
//!     attach "doc:<id>"       propose_customer_message's attach takes library documents too
//!                             (outbound_files: checked on the card, stamped at the send)
//! ```
//!
//! The hard rules live here, in code, and are checked when the card is made AND again at the send:
//! - never a `price_list` document (the internal price list), whoever asks;
//! - never a document whose audiences are internal only;
//! - a partner-only document (partner audience, no customer audience) only to a contact of a partner account;
//! - prices come only from the catalog: read_document hides money amounts (doc_tools.rs).

use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};

use super::actions::{apply_fn, entity_fn, need_actor, opt_uuid, Action, Field, Schema, Ty};
use super::outbound_files::{human_size, FileBlob};
use crate::doc_library::{self, access, Document, Filters, Scope, Version};

pub const ACTION: &str = "document.send";
/// A library document in an `attach` list: `{"type": "file", "id": "doc:<document id>"}`.
pub const PREFIX: &str = "doc:";
const PERMISSION: &str = "nary:approve";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> Option<String> {
    match args.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// `doc:<uuid>` (or a bare document id where only documents are meant: `bare`).
pub fn doc_ref(raw: &str) -> Option<Uuid> {
    let r = raw.trim();
    let id = r.strip_prefix(PREFIX)?;
    Uuid::parse_str(id.trim()).ok()
}

/// The attach id of a document.
pub fn attach_id(document_id: Uuid) -> String {
    format!("{PREFIX}{document_id}")
}

// ─── Who receives it ──────────────────────────────────────────────────────────

/// The person a document goes to, as far as the CRM knows them.
#[derive(Debug, Clone, Default)]
pub struct Recipient {
    pub contact_id: Option<Uuid>,
    /// The person (contact name, chat name or the address).
    pub name: Option<String>,
    /// Their organisation's name (the watermark's {company}).
    pub company: Option<String>,
    /// The partner account the contact belongs to, if any.
    pub partner_account_id: Option<Uuid>,
    pub partner_company: Option<String>,
    /// ISO 639-1, from their WhatsApp chat.
    pub language: Option<String>,
    /// whatsapp | email
    pub channel: String,
}

impl Recipient {
    pub fn is_partner(&self) -> bool {
        self.partner_account_id.is_some()
    }

    /// The company the copy is stamped for: the organisation (a partner's company), else the person.
    pub fn stamp_company(&self) -> String {
        self.company
            .clone()
            .or_else(|| self.partner_company.clone())
            .or_else(|| self.name.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Customer".to_string())
    }

    /// Readable: "Robert Smith (Acme Ltd)".
    pub fn label(&self) -> String {
        match (&self.name, &self.company) {
            (Some(n), Some(c)) if n != c => format!("{n} ({c})"),
            (Some(n), _) => n.clone(),
            (None, Some(c)) => c.clone(),
            (None, None) => "the customer".to_string(),
        }
    }

    /// The audiences a document needs one of to reach this person.
    pub fn audiences(&self) -> Vec<String> {
        let mut a = vec!["customer".to_string()];
        if self.is_partner() {
            a.push("partner".to_string());
        }
        a
    }
}

/// The partner account a contact belongs to: its organisation is a partner's, it is the partner's primary
/// contact, or its email is the partner's contact email.
pub async fn partner_of(
    db: &mut PgConnection,
    contact_id: Uuid,
) -> ApiResult<Option<(Uuid, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT pa.id, pa.company_name FROM partner_accounts pa JOIN contacts c ON c.id = $1 \
         WHERE pa.is_deleted = false AND (pa.primary_contact_id = c.id \
            OR (c.organization_id IS NOT NULL AND pa.organization_id = c.organization_id) \
            OR (coalesce(c.email, '') <> '' AND lower(pa.contact_email) = lower(c.email))) \
         ORDER BY pa.created_at, pa.id LIMIT 1",
    )
    .bind(contact_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// A recipient from a CRM contact (name, organisation, partner account, the language of their chat).
pub async fn recipient_for_contact(
    db: &mut PgConnection,
    contact_id: Option<Uuid>,
    channel: &str,
) -> ApiResult<Recipient> {
    let mut r = Recipient {
        channel: channel.to_string(),
        ..Default::default()
    };
    let Some(cid) = contact_id else {
        return Ok(r);
    };
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT c.first_name, c.last_name, o.name FROM contacts c \
         LEFT JOIN organizations o ON o.id = c.organization_id WHERE c.id = $1 AND c.is_deleted = false",
    )
    .bind(cid)
    .fetch_optional(&mut *db)
    .await?;
    let Some((first, last, org)) = row else {
        return Ok(r);
    };
    r.contact_id = Some(cid);
    let name = format!("{first} {}", last.unwrap_or_default())
        .trim()
        .to_string();
    r.name = Some(name).filter(|n| !n.is_empty());
    r.company = org.filter(|o| !o.trim().is_empty());
    if let Some((pid, pname)) = partner_of(db, cid).await? {
        r.partner_account_id = Some(pid);
        r.partner_company = pname.filter(|p| !p.trim().is_empty());
    }
    r.language = sqlx::query_scalar(
        "SELECT language FROM wa_chats WHERE contact_id = $1 AND coalesce(language, '') <> '' \
         ORDER BY last_inbound_at DESC NULLS LAST LIMIT 1",
    )
    .bind(cid)
    .fetch_optional(&mut *db)
    .await?;
    Ok(r)
}

/// A recipient from a WhatsApp chat (its contact, else the chat's name; the chat's language).
pub async fn recipient_for_chat(db: &mut PgConnection, chat_id: &str) -> ApiResult<Recipient> {
    let row: Option<(Option<Uuid>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT contact_id, display_name, language FROM wa_chats WHERE chat_id = $1",
    )
    .bind(chat_id)
    .fetch_optional(&mut *db)
    .await?;
    let (contact, display, language) = row.unwrap_or((None, None, None));
    let mut r = recipient_for_contact(db, contact, "whatsapp").await?;
    if r.name.is_none() {
        r.name = crate::nary::phones::clean_name(display.as_deref()).filter(|n| !n.is_empty());
    }
    if let Some(l) = language.filter(|l| !l.trim().is_empty()) {
        r.language = Some(l);
    }
    Ok(r)
}

/// A recipient of an email: the contact given, else the contact with that address, else the address.
pub async fn recipient_for_email(
    db: &mut PgConnection,
    to: &str,
    contact_id: Option<Uuid>,
) -> ApiResult<Recipient> {
    let contact =
        match contact_id {
            Some(c) => Some(c),
            None if to.contains('@') => sqlx::query_scalar(
                "SELECT id FROM contacts WHERE lower(email) = lower($1) AND is_deleted = false \
             ORDER BY created_at, id LIMIT 1",
            )
            .bind(to.trim())
            .fetch_optional(&mut *db)
            .await?,
            None => None,
        };
    let mut r = recipient_for_contact(db, contact, "email").await?;
    if r.name.is_none() && !to.trim().is_empty() {
        r.name = Some(to.trim().to_string());
    }
    Ok(r)
}

// ─── The hard rules ───────────────────────────────────────────────────────────

/// May this document go to this person? `Err(reason)` is told to the model or raised at the send.
pub fn may_send(doc: &Document, to: &Recipient) -> Result<(), String> {
    if doc.kind == "price_list" {
        return Err(format!(
            "{} is the company's internal price list: it never goes to a customer or partner. Prices come only from \
             the catalog (a quotation, or search_catalog for list prices).",
            doc.title
        ));
    }
    let customer = doc.has_audience("customer");
    let partner = doc.has_audience("partner");
    if !customer && !partner {
        return Err(format!(
            "{} is an internal document (its audience is internal only), so it is never sent to a customer.",
            doc.title
        ));
    }
    if !customer && !to.is_partner() {
        return Err(format!(
            "{} is for partners only, and {} is not a contact of a partner account.",
            doc.title,
            to.label()
        ));
    }
    Ok(())
}

/// A library document ready to send: the document and its live version.
#[derive(Debug, Clone)]
pub struct DocItem {
    pub doc: Document,
    pub version: Version,
}

impl DocItem {
    /// What a card lists: "Acme Gateway Brochure.pdf (Gateway Brochure v34, EN, 1.2 MB, stamped for Northwind)".
    pub fn label(&self, to: &Recipient) -> String {
        // 5 Nov 2026: a video is logged for the recipient, never watermarked
        if crate::doc_library::is_video(&self.version) {
            let mut parts = vec![
                format!("{} {}", self.doc.title, self.version.label),
                self.doc.language.to_uppercase(),
                human_size(self.version.file_size),
            ];
            if let Some(d) = self.version.duration_seconds {
                parts.push(crate::nary::uploads::duration_label(d));
            }
            parts.push(format!(
                "a video: logged for {}, not watermarked",
                to.stamp_company()
            ));
            return format!("{} ({})", self.version.file_name, parts.join(", "));
        }
        format!(
            "{} ({} {}, {}, {}, stamped for {})",
            self.version.file_name,
            self.doc.title,
            self.version.label,
            self.doc.language.to_uppercase(),
            human_size(self.version.file_size),
            to.stamp_company()
        )
    }
}

/// A document id to send to `to` as `user`: it exists, it is not the price list, the person may see it, it
/// has a live version, and the rules allow it for this recipient.
pub async fn resolve(
    db: &mut PgConnection,
    user: &AuthUser,
    document_id: Uuid,
    to: Option<&Recipient>,
) -> ApiResult<Result<DocItem, String>> {
    let Some(doc) = doc_library::load_document(db, document_id).await? else {
        return Ok(Err(format!(
            "No library document with id {document_id} (search_documents gives the ids)"
        )));
    };
    if doc.kind == "price_list" {
        // before anything else: the price list is never even looked at for a send
        return Ok(Err(may_send(&doc, &Recipient::default()).unwrap_err()));
    }
    if !access::can_view(db, user, &doc).await? {
        return Ok(Err(format!(
            "No library document with id {document_id} (search_documents gives the ids)"
        )));
    }
    let Some(to) = to else {
        return Ok(Err(format!(
            "{} can only go to a known customer: send it in a WhatsApp or email message to them.",
            doc.title
        )));
    };
    if let Err(why) = may_send(&doc, to) {
        return Ok(Err(why));
    }
    let Some(version) = doc_library::current_live(db, doc.id).await? else {
        return Ok(Err(format!(
            "{} has no live version yet (a new version waits for approval), so it can't be sent.",
            doc.title
        )));
    };
    Ok(Ok(DocItem { doc, version }))
}

/// A batch 4 catalog product file that the library took over (same stored object): its document.
pub async fn migrated_document(
    db: &mut PgConnection,
    attachment_id: Uuid,
) -> ApiResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT v.document_id FROM attachments a \
           JOIN library_document_versions v ON v.file_object_key = a.object_key \
           JOIN library_documents d ON d.id = v.document_id AND d.is_deleted = false \
          WHERE a.id = $1 AND a.entity_type = 'catalog_product' \
          ORDER BY v.created_at DESC LIMIT 1",
    )
    .bind(attachment_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// The customer's copy: stamped with their company and a fresh fingerprint (library_downloads, nary_send).
pub async fn customer_blob(
    db: &mut PgConnection,
    user: &AuthUser,
    item: &DocItem,
    to: &Recipient,
) -> ApiResult<FileBlob> {
    // the rules once more, right at the send
    if let Err(why) = may_send(&item.doc, to) {
        return Err(ApiError::http(403, why));
    }
    use crate::doc_library::deliver;
    let r = deliver::Recipient {
        company: Some(to.stamp_company()),
        name: to.name.clone(),
        partner_account_id: to.partner_account_id,
    };
    let channel = if to.channel.is_empty() {
        "whatsapp"
    } else {
        to.channel.as_str()
    };
    Ok(deliver::customer_copy_for(
        db,
        &item.version,
        &r,
        channel,
        deliver::Sender::Nary {
            confirmed_by: Some(user.id),
        },
    )
    .await?
    .file)
}

/// The footer line the customer's copy will carry (the setting, filled as far as it is known before the
/// send; the id is issued at the send).
pub async fn watermark_preview(db: &mut PgConnection, to: &Recipient) -> ApiResult<String> {
    use crate::doc_library::fingerprint::{preview_footer, Kind, Who};
    let who = Who {
        name: to.name.clone(),
        company: Some(to.stamp_company()),
        partner_account_id: to.partner_account_id,
        channel: Some(to.channel.clone()),
        ..Default::default()
    };
    preview_footer(db, Kind::NarySend, &who).await
}

// ─── Picking the document ─────────────────────────────────────────────────────

/// Product ids and families named by a product argument: an id, SKU, name (words), or a family.
pub async fn products_named(
    db: &mut PgConnection,
    raw: &str,
) -> ApiResult<(Vec<Uuid>, Vec<String>)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok((vec![], vec![]));
    }
    if let Ok(id) = Uuid::parse_str(raw) {
        return Ok((vec![id], vec![]));
    }
    let like = format!(
        "%{}%",
        raw.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM product_catalog WHERE is_deleted = false \
           AND (lower(sku) = lower($1) OR name ILIKE $2 OR short_name ILIKE $2) ORDER BY sku LIMIT 20",
    )
    .bind(raw)
    .bind(&like)
    .fetch_all(&mut *db)
    .await?;
    let families: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT family FROM ( \
            SELECT family FROM product_catalog WHERE is_deleted = false AND family ILIKE $1 \
            UNION SELECT family FROM library_document_products WHERE family ILIKE $1) f \
         WHERE family IS NOT NULL",
    )
    .bind(&like)
    .fetch_all(&mut *db)
    .await?;
    Ok((ids, families))
}

/// The products of a deal: its quotations' catalog lines, the catalog items of its product lines, and the
/// items or families its product interests name.
pub async fn deal_products(
    db: &mut PgConnection,
    deal_id: Uuid,
) -> ApiResult<(Vec<Uuid>, Vec<String>)> {
    let row: Option<(Option<Vec<String>>, Option<Vec<String>>)> = sqlx::query_as(
        "SELECT product_interests, product_line_ids FROM deals WHERE id = $1 AND is_deleted = false",
    )
    .bind(deal_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((interests, lines)) = row else {
        return Ok((vec![], vec![]));
    };
    let mut ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT qi.product_id FROM quotation_items qi JOIN quotations q ON q.id = qi.quotation_id \
         WHERE q.deal_id = $1 AND q.is_deleted = false AND qi.product_id IS NOT NULL",
    )
    .bind(deal_id)
    .fetch_all(&mut *db)
    .await?;
    let lines: Vec<String> = lines.unwrap_or_default();
    if !lines.is_empty() {
        let more: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM product_catalog WHERE is_deleted = false AND product_line_id::text = ANY($1)",
        )
        .bind(&lines)
        .fetch_all(&mut *db)
        .await?;
        ids.extend(more);
    }
    let mut families: Vec<String> = Vec::new();
    for i in interests.unwrap_or_default() {
        let (more, fams) = products_named(db, &i).await?;
        ids.extend(more);
        families.extend(fams);
    }
    if !ids.is_empty() {
        let fams: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT family FROM product_catalog WHERE id = ANY($1) AND family IS NOT NULL",
        )
        .bind(&ids)
        .fetch_all(&mut *db)
        .await?;
        families.extend(fams);
    }
    ids.sort();
    ids.dedup();
    families.sort();
    families.dedup();
    Ok((ids, families))
}

/// The words of a query (lower-cased, at least two characters).
fn words(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Every word in the title, description or file name.
fn matches_words(item: &DocItem, ws: &[String]) -> bool {
    let hay = format!(
        "{} {} {}",
        item.doc.title,
        item.doc.description.clone().unwrap_or_default(),
        item.version.file_name
    )
    .to_lowercase();
    ws.iter().all(|w| hay.contains(w.as_str()))
}

/// Out of language variants, the one in `wanted`, else English, else the first.
pub fn pick_language<'a>(variants: &'a [DocItem], wanted: Option<&str>) -> Option<&'a DocItem> {
    let want = wanted
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty());
    if let Some(w) = want {
        if let Some(v) = variants.iter().find(|v| v.doc.language.to_lowercase() == w) {
            return Some(v);
        }
    }
    variants
        .iter()
        .find(|v| v.doc.language.eq_ignore_ascii_case("en"))
        .or_else(|| variants.first())
}

/// The documents for these products/families that may go to `to` (and that `user` may see), with a live
/// version, narrowed by the words of `query`.
pub async fn candidates(
    db: &mut PgConnection,
    user: &AuthUser,
    to: &Recipient,
    products: &[Uuid],
    families: &[String],
    query: Option<&str>,
) -> ApiResult<Vec<DocItem>> {
    let scope = Scope::Audiences(to.audiences());
    let mut docs: Vec<Document> = Vec::new();
    let mut runs: Vec<Filters> = Vec::new();
    for p in products {
        runs.push(Filters {
            product_id: Some(*p),
            only_live: true,
            limit: Some(200),
            ..Default::default()
        });
    }
    for f in families {
        runs.push(Filters {
            family: Some(f.clone()),
            only_live: true,
            limit: Some(200),
            ..Default::default()
        });
    }
    if products.is_empty() && families.is_empty() {
        runs.push(Filters {
            only_live: true,
            limit: Some(200),
            ..Default::default()
        });
    }
    for f in runs {
        for d in doc_library::search(db, &scope, None, &f).await?.items {
            if !docs.iter().any(|x| x.id == d.id) {
                docs.push(d);
            }
        }
    }
    let ws = query.map(words).unwrap_or_default();
    let mut out = Vec::new();
    for d in docs {
        if d.kind == "price_list" || may_send(&d, to).is_err() {
            continue;
        }
        if !access::can_view(db, user, &d).await? {
            continue;
        }
        let Some(version) = doc_library::current_live(db, d.id).await? else {
            continue;
        };
        let item = DocItem { doc: d, version };
        if matches_words(&item, &ws) {
            out.push(item);
        }
    }
    Ok(out)
}

/// One document for the customer: the candidates grouped by language group, one variant per group in the
/// customer's language (else English). `Err` when none, or when several different documents fit (the
/// model then names one).
pub async fn auto_pick(
    db: &mut PgConnection,
    user: &AuthUser,
    to: &Recipient,
    products: &[Uuid],
    families: &[String],
    query: Option<&str>,
    language: Option<&str>,
) -> ApiResult<Result<DocItem, Value>> {
    let items = candidates(db, user, to, products, families, query).await?;
    let mut groups: Vec<(Uuid, Vec<DocItem>)> = Vec::new();
    for it in items {
        let key = it.doc.language_group_id.unwrap_or(it.doc.id);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(it),
            None => groups.push((key, vec![it])),
        }
    }
    let wanted = language.or(to.language.as_deref());
    let picked: Vec<DocItem> = groups
        .iter()
        .filter_map(|(_, variants)| pick_language(variants, wanted).cloned())
        .collect();
    match picked.len() {
        0 => Ok(Err(json!({"error": format!(
            "No library document for {} fits{}: none is linked to these products with a live version for \
             customers. search_documents lists what there is.",
            to.label(),
            query.map(|q| format!(" '{q}'")).unwrap_or_default()
        )}))),
        1 => Ok(Ok(picked.into_iter().next().unwrap())),
        _ => Ok(Err(json!({
            "error": "Several documents fit: call again with document_id (or narrower words in query).",
            "candidates": picked.iter().map(|p| json!({
                "document_id": p.doc.id.to_string(), "title": p.doc.title, "version": p.version.label,
                "language": p.doc.language, "file_name": p.version.file_name,
            })).collect::<Vec<_>>(),
        }))),
    }
}

// ─── The action ───────────────────────────────────────────────────────────────

fn schema() -> Schema {
    Schema::strict(
        "DocumentSend",
        vec![
            Field::req("document_id", Ty::Str),
            Field::req("version_id", Ty::Str),
            Field::req("channel", Ty::Literal(&["email", "whatsapp"])),
            Field::opt("chat_id", Ty::Str),
            Field::opt("to", Ty::Str),
            Field::opt("thread_id", Ty::Str),
            Field::opt("subject", Ty::Str),
            Field::opt("contact_id", Ty::Str),
            Field::opt("deal_id", Ty::Str),
            // R&D 26 Oct: copies, blind copies and the CRM inbox it goes from
            Field::late("cc", Ty::list(Ty::Str)),
            Field::late("bcc", Ty::list(Ty::Str)),
            Field::late("from_inbox", Ty::Str),
            Field::req("text", Ty::Str),
        ],
    )
}

pub fn actions() -> Vec<Action> {
    vec![Action::new(
        ACTION,
        "Send a library document to a customer",
        schema(),
        apply_fn(|db, a, p| Box::pin(_apply(db, a, p))),
    )
    .permission(PERMISSION)
    .describe(apply_fn(|db, a, p| Box::pin(_describe(db, a, p))))
    .entity(entity_fn(|p| match opt_uuid(p, "contact_id")? {
        Some(id) => Ok((Some("contact".into()), Some(id))),
        None => Ok((None, None)),
    }))]
}

fn s<'a>(p: &'a Value, k: &str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The recipient of a payload (its chat, else its address or contact).
async fn payload_recipient(db: &mut PgConnection, p: &Value) -> ApiResult<Recipient> {
    let contact = opt_uuid(p, "contact_id")?;
    if s(p, "channel") == "whatsapp" {
        let mut r = recipient_for_chat(db, s(p, "chat_id")).await?;
        if r.contact_id.is_none() && contact.is_some() {
            r = recipient_for_contact(db, contact, "whatsapp").await?;
        }
        return Ok(r);
    }
    recipient_for_email(db, s(p, "to"), contact).await
}

async fn _describe(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let title = match Uuid::parse_str(s(p, "document_id")) {
        Ok(id) => doc_library::load_document(db, id)
            .await?
            .map(|d| d.title)
            .unwrap_or_else(|| "a document".into()),
        Err(_) => "a document".into(),
    };
    let version = match Uuid::parse_str(s(p, "version_id")) {
        Ok(id) => doc_library::load_version(db, id)
            .await?
            .map(|v| v.label)
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    let to = payload_recipient(db, p).await?;
    let channel = if s(p, "channel") == "email" {
        "email"
    } else {
        "WhatsApp"
    };
    Ok(json!({
        "label": format!("Send {title} {version} to {} on {channel}", to.label()).replace("  ", " "),
        "after": {"text": s(p, "text")},
    }))
}

/// Sends the covering text and the stamped copy (batch 4's WhatsApp document / email attachment paths).
async fn _apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let user = need_actor(actor)?;
    let doc_id = Uuid::parse_str(s(p, "document_id"))
        .map_err(|_| ApiError::http(422, "document_id is not a valid id"))?;
    let to = payload_recipient(db, p).await?;
    let item = match resolve(db, user, doc_id, Some(&to)).await? {
        Ok(i) => i,
        Err(why) => return Err(ApiError::http(403, why)),
    };
    if item.version.id.to_string() != s(p, "version_id") {
        return Err(ApiError::http(
            409,
            format!(
                "{} has a newer live version ({}) than the card showed; prepare the card again.",
                item.doc.title, item.version.label
            ),
        ));
    }
    let mut reply = Map::new();
    for k in [
        "channel",
        "chat_id",
        "contact_id",
        "thread_id",
        "to",
        "subject",
        "cc",
        "bcc",
        "from_inbox",
        "text",
    ] {
        if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
            reply.insert(k.into(), v.clone());
        }
    }
    if s(p, "channel") == "email" && s(p, "thread_id").is_empty() && s(p, "subject").is_empty() {
        reply.insert("subject".into(), json!(item.doc.title));
    }
    reply.insert("by_owner".into(), json!(true));
    reply.insert(
        "attach".into(),
        json!([{"type": "file", "id": attach_id(item.doc.id)}]),
    );
    let out = super::channel_actions::send_reply(db, Some(user), &Value::Object(reply)).await?;
    let mut out = match out {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    out.insert(
        "label".into(),
        json!(format!(
            "Sent {} {} to {}",
            item.doc.title,
            item.version.label,
            to.label()
        )),
    );
    out.insert("document_id".into(), json!(item.doc.id.to_string()));
    out.insert("version".into(), json!(item.version.label));
    Ok(Value::Object(out))
}

// ─── The tool ─────────────────────────────────────────────────────────────────

/// Where the message goes: (payload fields, recipient), or the reason it can't go.
async fn destination(
    db: &mut PgConnection,
    args: &Value,
    channel: &str,
) -> ApiResult<Result<(Map<String, Value>, Recipient), String>> {
    let mut out = Map::new();
    out.insert("channel".into(), json!(channel));
    let contact_arg = match text(args, "contact_id") {
        Some(c) => match Uuid::parse_str(&c) {
            Ok(id) => Some(id),
            Err(_) => return Ok(Err("contact_id is not a valid id (search_contacts).".into())),
        },
        None => None,
    };
    if channel == "whatsapp" {
        let Some(chat_id) = text(args, "chat_id") else {
            return Ok(Err(
                "A WhatsApp send needs chat_id (list_recent_whatsapp).".into()
            ));
        };
        let chat: Option<granary_db::rows::nary::WaChat> =
            sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
                .bind(&chat_id)
                .fetch_optional(&mut *db)
                .await?;
        let Some(chat) = chat else {
            return Ok(Err(format!(
                "Not sent: Nary doesn't know the chat {chat_id} (the customer has to have written)."
            )));
        };
        if chat.status == "held" {
            return Ok(Err("Not sent: this chat waits for the Owner's business-or-personal decision (Nary → Intake → Held); decide it first.".into()));
        }
        if let Err(e) = crate::nary::wa_send::check(db, Some(&chat), "reply", true).await {
            if e.is_send_refused() {
                return Ok(Err(format!("Not sent: {}", e.detail())));
            }
            return Err(match e {
                crate::nary::wa_send::SendError::Api(a) => a,
                other => ApiError::internal(anyhow::anyhow!(other.to_string())),
            });
        }
        out.insert("chat_id".into(), json!(chat.chat_id));
        let mut to = recipient_for_chat(db, &chat.chat_id).await?;
        if to.contact_id.is_none() && contact_arg.is_some() {
            let lang = to.language.clone();
            to = recipient_for_contact(db, contact_arg, "whatsapp").await?;
            to.language = lang.or(to.language);
        }
        if let Some(c) = to.contact_id {
            out.insert("contact_id".into(), json!(c.to_string()));
        }
        return Ok(Ok((out, to)));
    }
    let mut contact = contact_arg;
    let address = if let Some(tid) = text(args, "thread_id") {
        let thread: Option<(Uuid, Option<Uuid>)> = match Uuid::parse_str(&tid) {
            Ok(id) => {
                sqlx::query_as("SELECT id, linked_contact_id FROM crm_email_threads WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *db)
                    .await?
            }
            Err(_) => None,
        };
        let Some((tid, linked)) = thread else {
            return Ok(Err(
                "No such email conversation; use search_emails to find its thread_id.".into(),
            ));
        };
        let last: Option<Option<String>> = sqlx::query_scalar(
            "SELECT from_email FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' \
             ORDER BY received_at DESC LIMIT 1",
        )
        .bind(tid)
        .fetch_optional(&mut *db)
        .await?;
        let Some(Some(from)) = last else {
            return Ok(Err("That conversation has no email from the customer to reply to; send a new email with `to`.".into()));
        };
        out.insert("thread_id".into(), json!(tid.to_string()));
        contact = contact.or(linked);
        // R&D 26 Oct: a subject of its own (it still threads) or the thread's real one; Cc everyone on it
        let subject = match text(args, "subject") {
            Some(s) => s,
            None => {
                crate::crm_inbox::reply_subject(&crate::crm_inbox::thread_subject(db, tid).await?)
            }
        };
        out.insert("subject".into(), json!(subject));
        let cc = match crate::ai::tools::catalog::email_list(args, "cc") {
            Some(c) => c,
            None => crate::crm_inbox::thread_participants(db, tid, &from)
                .await
                .map_err(ApiError::from)?,
        };
        out.insert("cc".into(), json!(cc));
        from
    } else {
        let Some(to) = text(args, "to").filter(|t| t.contains('@')) else {
            return Ok(Err("An email needs `to` (an address) or thread_id.".into()));
        };
        if let Some(subj) = text(args, "subject") {
            out.insert("subject".into(), json!(subj));
        }
        out.insert(
            "cc".into(),
            json!(crate::ai::tools::catalog::email_list(args, "cc").unwrap_or_default()),
        );
        to
    };
    out.insert(
        "bcc".into(),
        json!(crate::ai::tools::catalog::email_list(args, "bcc").unwrap_or_default()),
    );
    // the CRM inbox it goes from (named, else the rule), shown on the card
    let thread = out
        .get("thread_id")
        .and_then(Value::as_str)
        .and_then(|t| Uuid::parse_str(t).ok());
    let deal = text(args, "deal_id").and_then(|d| Uuid::parse_str(&d).ok());
    let named = text(args, "from_inbox");
    match crate::crm_mailboxes::pick(db, named.as_deref(), thread, deal).await {
        Ok(m) => {
            out.insert("from_inbox".into(), json!(m.address));
        }
        Err(e) => return Ok(Err(e.to_string())),
    }
    out.insert("to".into(), json!(address));
    let to = recipient_for_email(db, &address, contact).await?;
    if let Some(c) = to.contact_id {
        out.insert("contact_id".into(), json!(c.to_string()));
    }
    Ok(Ok((out, to)))
}

/// `propose_document_send`: a card that sends a library document (stamped for the customer) with a covering
/// text; nothing goes out until the user confirms.
pub async fn _h_propose_document_send(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let channel = text(&args, "channel").unwrap_or_default();
    if channel != "whatsapp" && channel != "email" {
        return err("channel must be whatsapp or email");
    }
    let Some(message) = text(&args, "text") else {
        return err(
            "The covering text is empty: write the short message that goes with the document.",
        );
    };
    if crate::ai::tools::catalog::_PLACEHOLDER.is_match(&message) {
        return err("The text still has a placeholder such as [Name]. Write the final text; Nary's signature is added automatically.");
    }
    let (mut payload, to) = match destination(db, &args, &channel).await? {
        Ok(d) => d,
        Err(why) => return err(why),
    };
    let deal_id = match text(&args, "deal_id") {
        Some(d) => match Uuid::parse_str(&d) {
            Ok(id) => Some(id),
            Err(_) => return err("deal_id is not a valid id (list_deals)."),
        },
        None => None,
    };
    let item = if let Some(raw) = text(&args, "document_id") {
        let id = doc_ref(&raw).or_else(|| Uuid::parse_str(&raw).ok());
        let Some(id) = id else {
            return err("document_id is not a valid id (search_documents gives the ids).");
        };
        match resolve(db, user, id, Some(&to)).await? {
            Ok(i) => i,
            Err(why) => return err(why),
        }
    } else {
        let (mut products, mut families) = match text(&args, "product") {
            Some(p) => products_named(db, &p).await?,
            None => (vec![], vec![]),
        };
        if let Some(d) = deal_id {
            let (p, f) = deal_products(db, d).await?;
            products.extend(p);
            families.extend(f);
        }
        if products.is_empty() && families.is_empty() {
            return err("Give document_id (search_documents), or deal_id or product to choose the document by.");
        }
        let query = text(&args, "query");
        let language = text(&args, "language");
        match auto_pick(
            db,
            user,
            &to,
            &products,
            &families,
            query.as_deref(),
            language.as_deref(),
        )
        .await?
        {
            Ok(i) => i,
            Err(v) => return Ok(v),
        }
    };
    payload.insert("document_id".into(), json!(item.doc.id.to_string()));
    payload.insert("version_id".into(), json!(item.version.id.to_string()));
    payload.insert("text".into(), json!(message));
    if let Some(d) = deal_id {
        payload.insert("deal_id".into(), json!(d.to_string()));
    }
    // the size cap of a message (batch 4)
    let (_, max_bytes) = super::outbound_files::limits(db).await?;
    if item.version.file_size > max_bytes {
        return err(format!(
            "{} is {}, over the {} a message may carry (nary.attach.max_total_mb).",
            item.version.file_name,
            human_size(item.version.file_size),
            human_size(max_bytes)
        ));
    }
    let watermark = watermark_preview(db, &to).await?;
    let channel_label = if channel == "email" {
        "email"
    } else {
        "WhatsApp"
    };
    let summary = format!(
        "Send {} {} to {} on {channel_label} as Nary",
        item.doc.title,
        item.version.label,
        to.label()
    );
    let details = vec![
        format!("Document: {} ({})", item.doc.title, item.version.file_name),
        format!("Version: {}", item.version.label),
        format!("Language: {}", item.doc.language.to_uppercase()),
        format!("Watermark: {watermark}"),
        format!("To: {} ({channel_label})", to.label()),
        format!("Message: {message}"),
    ];
    let mut details = details;
    if channel == "email" {
        let list = |k: &str| {
            payload
                .get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        let mut lines = vec![format!(
            "From: {}",
            payload
                .get("from_inbox")
                .and_then(Value::as_str)
                .unwrap_or("")
        )];
        let subject = payload
            .get("subject")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| item.doc.title.clone());
        lines.push(format!("Subject: {subject}"));
        if !list("cc").is_empty() {
            lines.push(format!("Cc: {}", list("cc")));
        }
        if !list("bcc").is_empty() {
            lines.push(format!("Bcc: {}", list("bcc")));
        }
        details.splice(4..4, lines);
    }
    let mut out = crate::ai::tools::catalog::_autonomy_card(
        db,
        user,
        ACTION,
        Value::Object(payload),
        summary,
        None,
    )
    .await?;
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        card.insert("details".into(), json!(details));
        card.insert(
            "document".into(),
            json!({
                "id": item.doc.id.to_string(), "title": item.doc.title, "version": item.version.label,
                "language": item.doc.language, "file_name": item.version.file_name,
                "size": human_size(item.version.file_size),
            }),
        );
        card.insert("watermark".into(), json!(watermark));
        card.insert("recipient".into(), json!(to.label()));
    }
    Ok(out)
}
