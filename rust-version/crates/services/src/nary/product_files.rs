//! 5 Nov 2026 (Stream A, Owner-approved plan): the Owner adds a file to a catalog product's Product files from the
//! ERP chat or the MCP client, always as a card the Owner confirms.
//!
//! ```text
//!     catalog.product_file   product_id, file_id (any id outbound_files::find_file resolves: a CRM attachment,
//!                            a file sent in the chat, an email attachment of the CRM inbox, an AI knowledge
//!                            document), title, audience customer|partner|internal, language, document_id
//!                            (a new version of that document instead of a new document), remove_from_record
//! ```
//!
//! A catalog item's Product files are document library documents (category products, linked to the product;
//! migration 0117). The file must be a PDF, an image or a video (MP4, MOV, WebM, judged by its bytes), within
//! documents.max_file_mb (videos nary.upload.video_max_mb). Store once: a CRM attachment's object (assets bucket,
//! or a Nary customer file under nary/files/) is referenced by the version, not copied (file_bucket); any other
//! file is copied into the library. Deleting the attachment later keeps the object (attachments delete_one).
//! remove_from_record takes the source attachment's ROW off its record after applying; the object stays.
//!
//! Only the Owner (the holder of the SOP role Owner; a superadmin for tests), at the tool and again at apply. Starts
//! at Ask, is on nary.autonomy.never_promote and nary.owner.confirm_twice_actions (migration 0162); over MCP it
//! never applies in one call (mcp_access::CATALOG_ACTIONS). Applied by the Owner, the version goes live at once:
//! the confirmed card is the approval (recorded with the Owner as approver).

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, Action, Field, Schema, Ty};
use super::catalog_changes::{detail, is_owner};
use super::outbound_files::{self as of, human_size, FileRef, Stored};
use super::uploads;
use crate::doc_library::{
    self as lib,
    versions::{self, NewDocument, NewFile, NewVersion, ProductRef, StoredFile, VersionOpts},
};

pub const ACTION: &str = "catalog.product_file";
pub const ONLY_OWNER: &str =
    "Only the Owner can add product files through Nary or the MCP client (Sales -> Catalog -> \
     an item -> Product files is the editor)";
/// The library category of product files.
pub const CATEGORY: &str = "products";
pub const AUDIENCE_TEXT: &str = "audience is customer, partner or internal";

fn schema() -> Schema {
    Schema::strict(
        "ProductFile",
        vec![
            Field::req("product_id", Ty::Str),
            Field::req("file_id", Ty::Str),
            Field::req("title", Ty::Str),
            Field::req("audience", Ty::Str),
            Field::req("language", Ty::Str),
            Field::opt("document_id", Ty::Str),
            Field::opt("remove_from_record", Ty::Bool),
        ],
    )
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        ACTION,
        "Add a file to a catalog product's Product files (the Owner only)",
        schema(),
        apply_fn(|db, a, p| Box::pin(apply(db, a, p))),
    )
    .describe(apply_fn(|db, a, p| Box::pin(describe(db, a, p))))]
}

/// What the card shows and what applying needs, worked out from the payload.
pub struct Checked {
    pub product_id: Uuid,
    pub product_label: String,
    pub file: FileRef,
    pub data: Vec<u8>,
    pub name: String,
    pub mime: String,
    /// pdf | image | video
    pub kind: &'static str,
    pub duration: Option<f64>,
    pub stored: Option<StoredFile>,
    /// a new version of this document (else a new document)
    pub document: Option<lib::Document>,
    pub title: String,
    pub audience: String,
    pub language: String,
    pub remove_from_record: bool,
    /// the record the source attachment is on ("deal", "lead"...), for a CRM attachment
    pub record: Option<(String, String)>,
}

fn s<'a>(p: &'a Value, k: &str) -> &'a str {
    p.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn record_word(etype: &str) -> &str {
    match etype {
        "crm_deal" => "deal",
        "crm_lead" => "lead",
        "crm_contact" => "contact",
        "crm_org" => "organization",
        "catalog_product" => "catalog product",
        "kb_article" => "Knowledge Base article",
        other => other,
    }
}

/// Every check of the card, against the tables as they are now. `Ok(Err(why))` is told to the model (or the
/// Owner at apply).
pub async fn check(
    db: &mut PgConnection,
    user: &AuthUser,
    p: &Value,
) -> ApiResult<Result<Checked, String>> {
    let Ok(product_id) = Uuid::parse_str(s(p, "product_id")) else {
        return Ok(Err(
            "product is required (its id, SKU or name; search_catalog finds it)".into(),
        ));
    };
    let prod: Option<(String, String)> = sqlx::query_as(
        "SELECT sku, name FROM product_catalog WHERE id = $1 AND is_deleted = false",
    )
    .bind(product_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((sku, pname)) = prod else {
        return Ok(Err(format!("No catalog product with id {product_id}")));
    };
    let audience = s(p, "audience").to_lowercase();
    let audience = if audience.is_empty() {
        "customer".to_string()
    } else {
        audience
    };
    if !lib::AUDIENCES.contains(&audience.as_str()) {
        return Ok(Err(format!(
            "Unknown audience '{audience}': {AUDIENCE_TEXT}"
        )));
    }
    let language = match versions::clean_language(Some(s(p, "language"))) {
        Ok(l) => l,
        Err(e) => return Ok(Err(detail(&e))),
    };
    let remove_from_record = p
        .get("remove_from_record")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // the file, as the person may open it
    let file = match of::find_file(db, user, s(p, "file_id")).await? {
        Ok(f) => f,
        Err(why) => return Ok(Err(why)),
    };
    if remove_from_record && file.source != "attachment" {
        return Ok(Err(format!(
            "remove_from_record works only for a file on a CRM record's Attachments; {} is not one",
            file.filename
        )));
    }
    let record: Option<(String, String)> = if file.source == "attachment" {
        sqlx::query_as("SELECT entity_type, entity_id FROM attachments WHERE id = $1")
            .bind(file.id)
            .fetch_optional(&mut *db)
            .await?
    } else {
        None
    };
    let data = match of::load_bytes(&file).await {
        Ok(d) => d,
        Err(e) => return Ok(Err(detail(&e))),
    };
    let probe = NewFile {
        data,
        file_name: file.filename.clone(),
        mime: Some(file.mime.clone()),
    };
    let (name, mime, kind) = match versions::check_file(db, &probe).await {
        Ok(x) => x,
        Err(e) => return Ok(Err(format!("{}: {}", file.filename, detail(&e)))),
    };
    let data = probe.data;
    let duration = if kind == "video" {
        file.duration.or_else(|| uploads::video_duration(&data))
    } else {
        None
    };
    // store once: a CRM attachment's own object (or a Nary customer file) is referenced, never copied
    let stored = match &file.stored {
        Stored::Object { bucket, key }
            if file.source == "attachment"
                || key.starts_with(crate::storage::NARY_FILES_PREFIX) =>
        {
            lib::bucket_value(bucket).map(|b| StoredFile {
                bucket: b,
                key: key.clone(),
            })
        }
        _ => None,
    };
    let document = match s(p, "document_id") {
        "" => None,
        raw => {
            let raw = raw.strip_prefix("doc:").unwrap_or(raw);
            let Ok(did) = Uuid::parse_str(raw) else {
                return Ok(Err(format!("'{raw}' is not a document id")));
            };
            match lib::load_document(db, did).await? {
                Some(d) if d.kind == "uploaded" => Some(d),
                Some(d) => {
                    return Ok(Err(format!(
                        "\"{}\" is made by the ERP; a file can't be added to it as a version",
                        d.title
                    )))
                }
                None => return Ok(Err(format!("No library document with id {did}"))),
            }
        }
    };
    let title = match s(p, "title") {
        "" => match name.rsplit_once('.') {
            Some((stem, _)) if !stem.trim().is_empty() => stem.trim().to_string(),
            _ => name.clone(),
        },
        t => t.to_string(),
    };
    Ok(Ok(Checked {
        product_id,
        product_label: format!("{sku} {pname}"),
        file,
        data,
        name,
        mime,
        kind,
        duration,
        stored,
        document,
        title: title.chars().take(300).collect(),
        audience,
        language,
        remove_from_record,
        record,
    }))
}

/// The card's summary and lines.
pub fn card_text(c: &Checked) -> (String, Vec<String>) {
    let mut file_bits = vec![
        uploads::type_label(&c.mime),
        human_size(c.data.len() as i64),
    ];
    if let Some(d) = c.duration {
        file_bits.push(uploads::duration_label(d));
    }
    let mut lines = vec![
        format!("Product: {}", c.product_label),
        format!("File: {} ({})", c.name, file_bits.join(", ")),
    ];
    lines.push(match &c.record {
        Some((etype, eid)) => format!("From: the Attachments of {} {eid}", record_word(etype)),
        None => format!("From: {}", c.file.source.replace('_', " ")),
    });
    lines.push(if c.stored.is_some() {
        "Stored once: the product file uses the same stored file (no copy)".to_string()
    } else {
        "Copied into the document library".to_string()
    });
    let summary = match &c.document {
        Some(d) => {
            lines.push(format!(
                "A new version of \"{}\" ({}, {}); its title, audience and language stay",
                d.title,
                d.audiences.join(", "),
                d.language.to_uppercase()
            ));
            format!(
                "Product file: new version of \"{}\" for {}",
                d.title, c.product_label
            )
        }
        None => {
            lines.push(format!(
                "A new document \"{}\" in Products, audience {}, language {}",
                c.title,
                c.audience,
                c.language.to_uppercase()
            ));
            format!("Product file \"{}\" for {}", c.title, c.product_label)
        }
    };
    lines.push("Goes live when you confirm: your confirmation is the approval".to_string());
    if c.kind == "video" {
        lines.push(format!(
            "A video: every copy sent is logged, never watermarked; {}",
            if c.mime.starts_with("video/mp4") {
                "on WhatsApp an MP4 up to nary.wa.video_max_mb plays as a video, a larger one goes as a document"
            } else {
                "on WhatsApp it goes as a document (WhatsApp plays only MP4)"
            }
        ));
    }
    if c.remove_from_record {
        let on = c
            .record
            .as_ref()
            .map(|(e, _)| format!("the {}'s", record_word(e)))
            .unwrap_or_else(|| "its record's".into());
        lines.push(format!(
            "Then taken off {on} Attachments (the stored file stays, the product file uses it)"
        ));
    }
    (summary, lines)
}

async fn describe(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let Some(actor) = actor else {
        return Ok(json!({"label": "Product file", "lines": []}));
    };
    Ok(match check(db, actor, p).await? {
        Ok(c) => {
            let (label, lines) = card_text(&c);
            json!({"label": label, "lines": lines})
        }
        Err(why) => json!({"label": "Product file",
                           "lines": [format!("Can't be applied as it stands: {why}")]}),
    })
}

/// Applies catalog.product_file. Commits.
async fn apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let actor = super::actions::need_actor(actor)?;
    if !is_owner(db, actor).await? {
        return Err(ApiError::http(403, ONLY_OWNER));
    }
    let c = match check(db, actor, p).await? {
        Ok(c) => c,
        Err(why) => return Err(ApiError::http(422, why)),
    };
    let source_note = match &c.record {
        Some((etype, eid)) => format!(
            "Added from the {} {eid} by the Owner's card",
            record_word(etype)
        ),
        None => "Added by the Owner's card".to_string(),
    };
    let file = NewFile {
        data: c.data,
        file_name: c.name.clone(),
        mime: Some(c.mime.clone()),
    };
    let opts = VersionOpts {
        stored: c.stored.clone(),
        card_approver: Some(actor.id),
    };
    let (doc_id, version) = match &c.document {
        Some(doc) => {
            let v = versions::add_version_with(
                db,
                actor,
                doc,
                NewVersion {
                    label: None,
                    change_note: source_note,
                    file,
                    source: None,
                },
                opts,
            )
            .await?;
            // the product link, when the document did not have it yet
            sqlx::query(
                "INSERT INTO library_document_products (id, document_id, product_id) \
                 SELECT $1, $2, $3 WHERE NOT EXISTS (SELECT 1 FROM library_document_products \
                 WHERE document_id = $2 AND product_id = $3)",
            )
            .bind(Uuid::new_v4())
            .bind(doc.id)
            .bind(c.product_id)
            .execute(&mut *db)
            .await?;
            (doc.id, v)
        }
        None => {
            let (d, v) = versions::create_document_with(
                db,
                actor,
                NewDocument {
                    title: c.title.clone(),
                    category: CATEGORY.into(),
                    audiences: vec![c.audience.clone()],
                    language: Some(c.language.clone()),
                    products: vec![ProductRef {
                        product_id: Some(c.product_id),
                        family: None,
                    }],
                    change_note: Some(source_note),
                    ..NewDocument::default()
                },
                file,
                None,
                opts,
            )
            .await?;
            (d.id, v)
        }
    };
    let mut removed = false;
    if c.remove_from_record && c.file.source == "attachment" {
        // only the listing goes; the object stays (the version uses it)
        let n = sqlx::query("DELETE FROM attachments WHERE id = $1")
            .bind(c.file.id)
            .execute(&mut *db)
            .await?
            .rows_affected();
        removed = n > 0;
    }
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         new_values, notes, origin) VALUES ($1, $2, $3, 'library.product_file', 'library_document', $4, $5, \
         $6::json, $7, 'nary')",
    )
    .bind(Uuid::new_v4())
    .bind(actor.id)
    .bind(&actor.name)
    .bind(doc_id.to_string())
    .bind(c.title.chars().take(200).collect::<String>())
    .bind(
        json!({"product_id": c.product_id.to_string(), "file_id": c.file.id.to_string(),
               "version_id": version.id.to_string(), "stored_once": c.stored.is_some(),
               "removed_from_record": removed})
        .to_string(),
    )
    .bind(format!("Product file for {}", c.product_label))
    .execute(&mut *db)
    .await?;
    super::commit(db).await?;
    let mut said = vec![format!(
        "{} is now a product file of {} ({}, {})",
        version.file_name, c.product_label, version.label, version.status
    )];
    if removed {
        said.push("taken off the record's Attachments; the stored file stays".into());
    }
    Ok(json!({
        "document_id": doc_id.to_string(),
        "version_id": version.id.to_string(),
        "file_id": crate::nary::document_send::attach_id(doc_id),
        "status": version.status,
        "stored_once": c.stored.is_some(),
        "removed_from_record": removed,
        "label": said.join("; "),
    }))
}
