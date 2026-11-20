//! Port of backend/app/services/documents/core.py (`core` is `core_` in Rust).
//!
//! Sales documents: create, edit, link, revise, fingerprint (Nary Phase 4 item A).
//!
//! A document is sections of ordered blocks. The section list and its headings come from the template
//! setting (documents.proposal.template), so the Owner edits the template, not the code. Block data by type:
//!
//! ```text
//!     text        {"md": "..."}                                 Markdown (rendered and sanitised)
//!     kv          {"rows": [[label, value], ...]}
//!     table       {"columns": [...], "rows": [[...], ...]}
//!     coverage    {"rows": [{"requirement", "status", "notes"}]}   status: included | optional_custom | not_in_scope | to_confirm
//!     image       {"asset_id": "...", "caption": "..."}
//!     commercial  {}                                             rendered from the linked quotations, never stored
//!     stamp       {}                                             the approval stamp
//!     chart       {"spec", "caption", "size", "brief", "svg_asset_id", "png_asset_id", "checked"}   (visuals V2)
//!     diagram     the same, with a diagram spec (figures.rs guards and draws both on save)
//! ```
//!
//! A revision copies the document and its blocks with revision + 1 (same revision_group), supersedes the
//! earlier one (status, approval, verification) and revises its quotations too, linking the new ones.
//!
//! SQLAlchemy unit-of-work notes: Python adds rows to the session and flushes; here each row is written
//! when Python would have added it (the router's transaction makes the difference invisible). ORM updates
//! also set `updated_at = now()` (the column's `onupdate`), so every UPDATE here does too.

use chrono::NaiveDate;
use granary_compat::pyjson::{self, DumpOpts, PyDecimal, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json, SettingError};
use granary_db::rows::crm::Deal;
use granary_db::rows::documents::{SalesDocument, SalesDocumentAsset, SalesDocumentBlock};
use granary_db::rows::sales::Quotation;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

/// SettingError prefixes "Settings → " itself.
pub const WHERE: &str = "Sales & CRM → Proposals";
pub const EDITABLE: &[&str] = &["draft", "review"];
/// app.models.documents.BLOCK_TYPES
pub const BLOCK_TYPES: &[&str] = &[
    "text",
    "kv",
    "table",
    "coverage",
    "image",
    "commercial",
    "stamp",
    // visuals V2: figures drawn from a spec (documents/figures.rs)
    "chart",
    "diagram",
];
/// app.models.documents.COVERAGE_STATUSES
pub const COVERAGE_STATUSES: &[&str] =
    &["included", "optional_custom", "not_in_scope", "to_confirm"];

// ------------------------------------------------------------------------- Python value helpers

/// Python truthiness of a JSON-decoded value.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `a or b` for JSON values.
pub(crate) fn or<'a>(a: Option<&'a Value>, b: &'a Value) -> &'a Value {
    match a {
        Some(v) if truthy(v) => v,
        _ => b,
    }
}

/// The type name Python would print in an AttributeError/TypeError.
fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `x.get(...)` on a value that must be a dict (AttributeError, a 500, otherwise).
pub(crate) fn as_dict<'a>(v: &'a Value, what: &str) -> ApiResult<&'a Map<String, Value>> {
    v.as_object().ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "AttributeError: '{}' object has no attribute 'get' ({what})",
            py_type(v)
        ))
    })
}

/// `for x in v`: a list's items, a string's characters, a dict's keys; anything else is a TypeError.
pub(crate) fn py_iter(v: &Value) -> ApiResult<Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "TypeError: '{}' object is not iterable",
            py_type(other)
        ))),
    }
}

/// `str(value)` of a JSON-decoded value.
pub(crate) fn py_str(v: &Value) -> String {
    pyjson::str_value(&PyValue::from(v))
}

/// `s[:n]` on a str (code points).
pub(crate) fn py_slice(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `uuid.UUID(str(x))`: braces, "urn:"/"uuid:" prefixes and hyphens anywhere are accepted, then 32 hex
/// digits. A malformed value is a ValueError (a 500 in Python).
pub(crate) fn py_uuid(s: &str) -> ApiResult<Uuid> {
    let h = s.replace("urn:", "").replace("uuid:", "");
    let h = h.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        if let Ok(u) = Uuid::parse_str(&h) {
            return Ok(u);
        }
    }
    Err(ApiError::internal(anyhow::anyhow!(
        "ValueError: badly formed hexadecimal UUID string"
    )))
}

/// A setting holding something the code then uses wrongly: the AttributeError/KeyError Python raises.
fn bad(msg: String) -> ApiError {
    ApiError::internal(anyhow::anyhow!(msg))
}

// ------------------------------------------------------------------------------------- settings

/// `template(db)`: the section list from documents.proposal.template.
pub async fn template(db: &mut PgConnection) -> ApiResult<Vec<Value>> {
    let t = require_json(db, "documents.proposal.template", WHERE).await?;
    let ok = match &t {
        Value::Array(a) if !a.is_empty() => a.iter().all(|s| {
            s.as_object()
                .map(|o| o.get("key").is_some_and(truthy) && o.get("title").is_some_and(truthy))
                .unwrap_or(false)
        }),
        _ => false,
    };
    if !ok {
        return Err(SettingError::new(
            "documents.proposal.template",
            "must be a list of {key, title, blocks, guidance}",
            WHERE,
        )
        .into());
    }
    // Titles are English text; an older stored {"en": ..} object reads as its English title.
    let mut sections = t.as_array().cloned().unwrap_or_default();
    for s in &mut sections {
        let key = s
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(title) = s.get("title") {
            if !title.is_string() {
                s["title"] = json!(section_title_text(Some(title)).unwrap_or(key));
            }
        }
    }
    Ok(sections)
}

/// A section title as text: a string as it is, an older {"en": ..} object as its English title.
pub fn section_title_text(title: Option<&Value>) -> Option<String> {
    match title? {
        Value::String(s) => Some(s.clone()),
        Value::Object(m) => m.get("en").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
    .filter(|t| !t.is_empty())
}

/// `labels(db, language)`: the label set for a language, English when it has none.
pub async fn labels(db: &mut PgConnection, language: &str) -> ApiResult<Value> {
    let all = require_json(db, "documents.proposal.labels", WHERE).await?;
    let empty = json!({});
    let all_ = or(Some(&all), &empty);
    // `x in all_` is a key test on a dict, a membership test on a list, a substring test on a str.
    let contains = |k: &str| -> bool {
        match all_ {
            Value::Object(o) => o.contains_key(k),
            Value::Array(a) => a.iter().any(|v| v.as_str() == Some(k)),
            Value::String(s) => s.contains(k),
            _ => false,
        }
    };
    let lang = if contains(language) { language } else { "en" };
    if !contains(lang) {
        return Err(
            SettingError::new("documents.proposal.labels", "has no English labels", WHERE).into(),
        );
    }
    match all_ {
        Value::Object(o) => Ok(o[lang].clone()),
        other => Err(bad(format!(
            "TypeError: {} indices must be integers",
            py_type(other)
        ))),
    }
}

/// `_empty(block_type)`: the data of a new block (KeyError for an unknown type).
pub fn _empty(block_type: &str) -> ApiResult<Value> {
    Ok(match block_type {
        "text" => json!({"md": ""}),
        "kv" => json!({"rows": []}),
        "table" => json!({"columns": [], "rows": []}),
        "coverage" => json!({"rows": []}),
        "image" => json!({"asset_id": null, "caption": ""}),
        "commercial" => json!({}),
        "stamp" => json!({}),
        other => return Err(bad(format!("KeyError: '{other}'"))),
    })
}

/// `validate_block(block_type, data)`. `block_type` is the JSON value given (None when missing).
/// One kv/table row as a list of text cells, or None when it is not a row (a number, a null...). A kv row needs
/// a label and a value; an object row {"label"/"key", "value"} becomes [label, value].
fn clean_row(bt: &str, r: Value) -> Option<Value> {
    let cells: Vec<Value> = match r {
        Value::Array(a) => a
            .into_iter()
            .map(|c| match c {
                Value::String(_) => c,
                Value::Null => Value::String(String::new()),
                other => Value::String(py_str(&other)),
            })
            .collect(),
        Value::Object(m) if bt == "kv" => {
            let label = m.get("label").or_else(|| m.get("key"))?;
            let value = m.get("value")?;
            vec![Value::String(py_str(label)), Value::String(py_str(value))]
        }
        _ => return None,
    };
    if bt == "kv" && cells.len() < 2 {
        return None;
    }
    (!cells.is_empty()).then_some(Value::Array(cells))
}

pub fn validate_block(block_type: &Value, data: &Value) -> ApiResult<Value> {
    let bt = block_type.as_str().filter(|b| BLOCK_TYPES.contains(b));
    let Some(bt) = bt else {
        return Err(ApiError::http(
            422,
            format!("Unknown block type '{}'", py_str(block_type)),
        ));
    };
    let empty = json!({});
    let data = or(Some(data), &empty);
    // 22 Oct: kv and table rows must be lists of cells (a kv row [label, value]); anything else is dropped
    // here, so a malformed row can never reach the PDF (a draft once stored rows [0, 0])
    if bt == "kv" || bt == "table" {
        let mut d = data.as_object().cloned().unwrap_or_default();
        let rows = d
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let clean: Vec<Value> = rows.into_iter().filter_map(|r| clean_row(bt, r)).collect();
        d.insert("rows".into(), Value::Array(clean));
        return Ok(Value::Object(d));
    }
    if bt == "coverage" {
        let d = as_dict(data, "coverage data")?;
        let no_rows = json!([]);
        for r in py_iter(or(d.get("rows"), &no_rows))? {
            let r = as_dict(&r, "coverage row")?;
            let ok = r
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| COVERAGE_STATUSES.contains(&s));
            if !ok {
                return Err(ApiError::http(
                    422,
                    format!(
                        "Coverage status must be one of {}",
                        COVERAGE_STATUSES.join(", ")
                    ),
                ));
            }
        }
    }
    if bt == "commercial" || bt == "stamp" {
        return Ok(json!({})); // rendered, never typed
    }
    if super::figures::is_figure(bt) {
        // visuals V2: the spec contract; drawing happens in replace_blocks
        return super::figures::validate_data(bt, data);
    }
    Ok(data.clone())
}

// ------------------------------------------------------------------------------------- create

/// The keyword arguments of `create` (Python defaults in `Default`). `user_id` is `getattr(user, "id", None)`.
#[derive(Debug, Clone)]
pub struct CreateArgs<'a> {
    pub user_id: Option<Uuid>,
    pub title: String,
    pub language: String,
    pub subtitle: Option<String>,
    pub deal: Option<&'a Deal>,
    pub organization_id: Option<Uuid>,
    pub contact_id: Option<Uuid>,
    /// None: the template's sections, each with its default blocks.
    pub blocks: Option<Vec<Value>>,
    pub quotation_ids: Vec<Uuid>,
    pub origin: String,
    pub cover: Option<Value>,
}

impl Default for CreateArgs<'_> {
    fn default() -> Self {
        CreateArgs {
            user_id: None,
            title: String::new(),
            language: "en".into(),
            subtitle: None,
            deal: None,
            organization_id: None,
            contact_id: None,
            blocks: None,
            quotation_ids: Vec::new(),
            origin: "manual".into(),
            cover: None,
        }
    }
}

const DOC_COLUMNS: &str = "doc_type, number, full_number, revision, revision_group, previous_version_id, revision_reason, \
     title, subtitle, language, deal_id, organization_id, contact_id, status, validity_days, classification, cover, \
     issue_date, created_by_id, sent_at, origin, id, created_at, updated_at, is_deleted, deleted_at";

/// `create(db, *, user, title, language, subtitle, deal, organization_id, contact_id, blocks, quotation_ids,
/// origin, cover)`.
pub async fn create(db: &mut PgConnection, args: CreateArgs<'_>) -> ApiResult<SalesDocument> {
    // Proposals are English.
    let _ = args.language;
    let language = "en";
    let number = crate::numbering::next_document_number(db, "proposal").await?;
    let cls_all = require_json(db, "documents.proposal.classification", WHERE).await?;
    let empty = json!({});
    let cls = as_dict(
        or(Some(&cls_all), &empty),
        "documents.proposal.classification",
    )?
    .get(language)
    .cloned();
    let classification = match cls {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(other) => {
            return Err(bad(format!(
                "classification must be a str, got {}",
                py_type(&other)
            )))
        }
    };
    let validity_days = require_int(db, "documents.proposal.validity_days", WHERE).await?;
    let subtitle = py_slice(args.subtitle.as_deref().unwrap_or(""), 500);
    let subtitle = if subtitle.is_empty() {
        None
    } else {
        Some(subtitle)
    };
    let cover = match &args.cover {
        Some(c) if truthy(c) => c.clone(),
        _ => json!({}),
    };
    let organization_id = args
        .organization_id
        .or(args.deal.and_then(|d| d.organization_id));
    let contact_id = args.contact_id.or(args.deal.and_then(|d| d.contact_id));
    let doc: SalesDocument = sqlx::query_as(&format!(
        "INSERT INTO sales_documents (id, doc_type, number, revision, revision_group, full_number, title, subtitle, \
         language, deal_id, organization_id, contact_id, status, validity_days, classification, cover, created_by_id, \
         origin, is_deleted) VALUES ($1, 'solution_proposal', $2, 1, $2, $3, $4, $5, $6, $7, $8, $9, 'draft', $10, $11, \
         $12, $13, $14, false) RETURNING {DOC_COLUMNS}"
    ))
    .bind(Uuid::new_v4())
    .bind(&number)
    .bind(format!("{number} Rev. 1"))
    .bind(py_slice(&args.title, 300))
    .bind(subtitle)
    .bind(language)
    .bind(args.deal.map(|d| d.id))
    .bind(organization_id)
    .bind(contact_id)
    .bind(validity_days as i32)
    .bind(classification)
    .bind(&cover)
    .bind(args.user_id)
    .bind(&args.origin)
    .fetch_one(&mut *db)
    .await?;
    match &args.blocks {
        None => {
            let mut pos = 0i32;
            let text = json!(["text"]);
            for s in template(db).await? {
                let s = as_dict(&s, "template section")?;
                for bt in py_iter(or(s.get("blocks"), &text))? {
                    let bt_s = bt
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| py_str(&bt));
                    let data = _empty(&bt_s)?;
                    insert_block(db, doc.id, &s["key"], pos, &bt_s, &data).await?;
                    pos += 1;
                }
            }
        }
        Some(blocks) => replace_blocks(db, &doc, blocks).await?,
    }
    for (i, qid) in args.quotation_ids.iter().enumerate() {
        insert_link(db, doc.id, *qid, i as i32, None).await?;
    }
    Ok(doc)
}

// fix-longapply: a proposal drafted again in place (the retry of a cut apply, a revision): never a duplicate.
/// Replaces a draft proposal's title, subtitle, language, cover and blocks (its number, deal and revision
/// stay). Only while it can be edited (draft or review).
pub async fn redraft(
    db: &mut PgConnection,
    doc_id: Uuid,
    title: &str,
    subtitle: &str,
    language: &str,
    cover: Value,
    blocks: &[Value],
) -> ApiResult<SalesDocument> {
    // Proposals are English.
    let _ = language;
    let language = "en";
    let subtitle = py_slice(subtitle, 500);
    let doc: Option<SalesDocument> = sqlx::query_as(&format!(
        "UPDATE sales_documents SET title = $2, subtitle = $3, language = $4, \
         cover = COALESCE(cover, '{{}}'::jsonb) || $5, updated_at = now() \
         WHERE id = $1 AND is_deleted = false AND status IN ('draft', 'review') RETURNING {DOC_COLUMNS}"
    ))
    .bind(doc_id)
    .bind(py_slice(title, 300))
    .bind(if subtitle.is_empty() { None } else { Some(subtitle) })
    .bind(language)
    .bind(&cover)
    .fetch_optional(&mut *db)
    .await?;
    let doc = doc.ok_or_else(|| {
        ApiError::http(
            409,
            "That proposal can't be drafted again: it is no longer a draft (revise it)",
        )
    })?;
    replace_blocks(db, &doc, blocks).await?;
    Ok(doc)
}

async fn insert_block(
    db: &mut PgConnection,
    document_id: Uuid,
    section: &Value,
    position: i32,
    block_type: &str,
    data: &Value,
) -> ApiResult<()> {
    let section = match section {
        Value::String(s) => s.clone(),
        other => {
            return Err(bad(format!(
                "section must be a str, got {}",
                py_type(other)
            )))
        }
    };
    sqlx::query(
        "INSERT INTO sales_document_blocks (id, document_id, section, position, block_type, data) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind(section)
    .bind(position)
    .bind(block_type)
    .bind(data)
    .execute(&mut *db)
    .await?;
    Ok(())
}

async fn insert_link(
    db: &mut PgConnection,
    document_id: Uuid,
    quotation_id: Uuid,
    position: i32,
    label: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO sales_document_quotations (id, document_id, quotation_id, position, label) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind(quotation_id)
    .bind(position)
    .bind(label)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `replace_blocks(db, doc, blocks)`: replace every block (the editor saves the whole document; positions
/// follow the list order). Each block is a dict {section, block_type | type, data}.
pub async fn replace_blocks(
    db: &mut PgConnection,
    doc: &SalesDocument,
    blocks: &[Value],
) -> ApiResult<()> {
    if !EDITABLE.contains(&doc.status.as_str()) {
        return Err(ApiError::http(
            409,
            format!("A {} proposal can't be edited: revise it", doc.status),
        ));
    }
    let keys: Vec<Value> = template(db)
        .await?
        .iter()
        .map(|s| s["key"].clone())
        .collect();
    sqlx::query("DELETE FROM sales_document_blocks WHERE document_id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    let null = Value::Null;
    let empty = json!({});
    for (pos, b) in blocks.iter().enumerate() {
        let b = as_dict(b, "block")?;
        let section = b.get("section").unwrap_or(&null);
        if !keys.contains(section) {
            return Err(ApiError::http(
                422,
                format!("Unknown section '{}'", py_str(section)),
            ));
        }
        let bt = or(b.get("block_type"), b.get("type").unwrap_or(&null));
        let mut data = validate_block(bt, or(b.get("data"), &empty))?;
        if let Some(fig) = bt.as_str().filter(|t| super::figures::is_figure(t)) {
            // visuals V2: guard and draw (unless its stored drawing already matches the spec)
            data = super::figures::materialise(db, doc, fig, &data).await?;
        }
        insert_block(
            db,
            doc.id,
            section,
            pos as i32,
            bt.as_str().unwrap_or_default(),
            &data,
        )
        .await?;
    }
    Ok(())
}

/// `blocks_of(db, doc_id)`: the blocks in position order.
pub async fn blocks_of(db: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<SalesDocumentBlock>> {
    Ok(sqlx::query_as(
        "SELECT document_id, section, position, block_type, data, id, created_at, updated_at \
         FROM sales_document_blocks WHERE document_id = $1 ORDER BY position",
    )
    .bind(doc_id)
    .fetch_all(&mut *db)
    .await?)
}

#[derive(sqlx::FromRow)]
struct QuotationWithLabel {
    #[sqlx(flatten)]
    q: Quotation,
    link_label__: Option<String>,
}

/// `quotations_of(db, doc_id)`: (quotation, label) in link position order.
pub async fn quotations_of(
    db: &mut PgConnection,
    doc_id: Uuid,
) -> ApiResult<Vec<(Quotation, Option<String>)>> {
    let cols = Quotation::COLUMNS
        .iter()
        .map(|c| format!("q.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let rows: Vec<QuotationWithLabel> = sqlx::query_as(&format!(
        "SELECT {cols}, l.label AS link_label__ FROM quotations q \
         JOIN sales_document_quotations l ON l.quotation_id = q.id \
         WHERE l.document_id = $1 ORDER BY l.position"
    ))
    .bind(doc_id)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.q, r.link_label__)).collect())
}

async fn get_quotation(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Quotation>> {
    let cols = Quotation::COLUMNS.join(", ");
    Ok(
        sqlx::query_as(&format!("SELECT {cols} FROM quotations WHERE id = $1"))
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `link_quotation(db, doc, quotation_id, label)`.
pub async fn link_quotation(
    db: &mut PgConnection,
    doc: &SalesDocument,
    quotation_id: &str,
    label: Option<&str>,
) -> ApiResult<()> {
    let qid = py_uuid(quotation_id)?;
    let q = match get_quotation(db, qid).await? {
        Some(q) if !q.is_deleted => q,
        _ => return Err(ApiError::http(404, "Quotation not found")),
    };
    if let (Some(dd), Some(qd)) = (doc.deal_id, q.deal_id) {
        if qd != dd {
            return Err(ApiError::http(
                400,
                "That quotation belongs to another deal",
            ));
        }
    }
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM sales_document_quotations WHERE document_id = $1 AND quotation_id = $2",
    )
    .bind(doc.id)
    .bind(q.id)
    .fetch_optional(&mut *db)
    .await?;
    if exists.is_none() {
        let n = quotations_of(db, doc.id).await?.len();
        insert_link(db, doc.id, q.id, n as i32, label).await?;
    }
    Ok(())
}

/// Rust only (22 Oct 2026, proposal.quotation): takes a quotation off a proposal; the others keep their order.
/// Returns whether it was linked.
pub async fn unlink_quotation(
    db: &mut PgConnection,
    doc: &SalesDocument,
    quotation_id: Uuid,
) -> ApiResult<bool> {
    let gone = sqlx::query(
        "DELETE FROM sales_document_quotations WHERE document_id = $1 AND quotation_id = $2",
    )
    .bind(doc.id)
    .bind(quotation_id)
    .execute(&mut *db)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE sales_document_quotations l SET position = r.n - 1 \
         FROM (SELECT id, row_number() OVER (ORDER BY position) AS n \
               FROM sales_document_quotations WHERE document_id = $1) r \
         WHERE l.id = r.id",
    )
    .bind(doc.id)
    .execute(&mut *db)
    .await?;
    Ok(gone > 0)
}

/// `str(x)` of an optional Decimal column (`str(None)` is "None").
pub fn str_dec(d: Option<rust_decimal::Decimal>) -> String {
    match d {
        Some(d) => PyDecimal::from(d).to_string(),
        None => "None".into(),
    }
}

/// What `fingerprint` reads of a linked quotation: (id, full_number or number, total, currency, revision).
pub type FingerprintQuote = (
    Uuid,
    String,
    Option<rust_decimal::Decimal>,
    Option<String>,
    Option<i32>,
);

/// The quotation part of the fingerprint.
pub fn fingerprint_quote(q: &Quotation) -> FingerprintQuote {
    let number = match &q.full_number {
        Some(f) if !f.is_empty() => f.clone(),
        _ => q.number.clone(),
    };
    (q.id, number, q.total, q.currency.clone(), q.revision)
}

/// The body `fingerprint` hashes (separate for the golden tests): the document's title, subtitle,
/// language, cover, validity_days, classification, its blocks as (section, block_type, data) and the
/// linked quotations.
#[allow(clippy::too_many_arguments)]
pub fn fingerprint_body(
    title: &str,
    subtitle: Option<&str>,
    language: &str,
    cover: &Value,
    validity_days: i32,
    classification: Option<&str>,
    blocks: &[(String, String, Value)],
    quotes: &[FingerprintQuote],
) -> PyValue {
    let opt_str = |s: Option<&str>| {
        s.map(|v| PyValue::Str(v.to_string()))
            .unwrap_or(PyValue::None)
    };
    let blocks = blocks
        .iter()
        .map(|(section, bt, data)| {
            PyValue::List(vec![
                PyValue::Str(section.clone()),
                PyValue::Str(bt.clone()),
                py_value_of(data),
            ])
        })
        .collect();
    let quotations = quotes
        .iter()
        .map(|(id, number, total, currency, revision)| {
            PyValue::List(vec![
                PyValue::Str(id.hyphenated().to_string()),
                PyValue::Str(number.clone()),
                PyValue::Str(str_dec(*total)),
                opt_str(currency.as_deref()),
                revision
                    .map(|r| PyValue::Int(r as i128))
                    .unwrap_or(PyValue::None),
            ])
        })
        .collect();
    PyValue::Dict(vec![
        ("title".into(), PyValue::Str(title.to_string())),
        ("subtitle".into(), opt_str(subtitle)),
        ("language".into(), PyValue::Str(language.to_string())),
        ("cover".into(), py_value_of(cover)),
        ("validity_days".into(), PyValue::Int(validity_days as i128)),
        ("classification".into(), opt_str(classification)),
        ("blocks".into(), PyValue::List(blocks)),
        ("quotations".into(), PyValue::List(quotations)),
    ])
}

const BIG_INT_MARK: &str = "\u{0}BIGINT:";

/// What json.loads makes of a jsonb value. Python ints are unbounded (a float like 1e300 comes back from
/// jsonb as a 301-digit integer); PyValue holds i128, so a larger integer travels as a marked string that
/// `dump_body` puts back as the bare digits. jsonb can't hold U+0000, so the mark can't clash with data.
pub fn py_value_of(v: &Value) -> PyValue {
    match v {
        Value::Number(n) => {
            let t = n.to_string();
            if !t.contains(['.', 'e', 'E']) && t.parse::<i128>().is_err() {
                PyValue::Str(format!("{BIG_INT_MARK}{t}\u{0}"))
            } else {
                PyValue::from(v)
            }
        }
        Value::Array(a) => PyValue::List(a.iter().map(py_value_of).collect()),
        Value::Object(o) => {
            PyValue::Dict(o.iter().map(|(k, x)| (k.clone(), py_value_of(x))).collect())
        }
        other => PyValue::from(other),
    }
}

/// `json.dumps(body, sort_keys=True, default=str)`.
pub fn dump_body(body: &PyValue) -> ApiResult<String> {
    let text = pyjson::dumps(body, &DumpOpts::sorted_default_str())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("json.dumps: {e:?}")))?;
    const OPEN: &str = "\"\\u0000BIGINT:";
    const CLOSE: &str = "\\u0000\"";
    if !text.contains(OPEN) {
        return Ok(text);
    }
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find(OPEN) {
        out.push_str(&rest[..i]);
        let after = &rest[i + OPEN.len()..];
        let j = after.find(CLOSE).unwrap_or(after.len());
        out.push_str(&after[..j]);
        rest = &after[(j + CLOSE.len()).min(after.len())..];
    }
    out.push_str(rest);
    Ok(out)
}

/// sha256 hex of `json.dumps(body, sort_keys=True, default=str)`.
pub fn hash_body(body: &PyValue) -> ApiResult<String> {
    let text = dump_body(body)?;
    Ok(hex::encode(Sha256::digest(text.as_bytes())))
}

/// `fingerprint(db, doc)`: sha256 of what the customer reads: cover, title, blocks in order, linked
/// quotations and their totals. Any edit changes it, so an approval given to an earlier text no longer stands.
pub async fn fingerprint(db: &mut PgConnection, doc: &SalesDocument) -> ApiResult<String> {
    let blocks = blocks_of(db, doc.id).await?;
    let quotes = quotations_of(db, doc.id).await?;
    let blocks: Vec<(String, String, Value)> = blocks
        .into_iter()
        .map(|b| (b.section, b.block_type, b.data))
        .collect();
    let quotes: Vec<FingerprintQuote> = quotes.iter().map(|(q, _)| fingerprint_quote(q)).collect();
    hash_body(&fingerprint_body(
        &doc.title,
        doc.subtitle.as_deref(),
        &doc.language,
        &doc.cover,
        doc.validity_days,
        doc.classification.as_deref(),
        &blocks,
        &quotes,
    ))
}

async fn assets_of(db: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<SalesDocumentAsset>> {
    let cols = SalesDocumentAsset::COLUMNS.join(", ");
    // No ORDER BY, as in Python: the copies are paired with the originals by heap order (see `revise`).
    Ok(sqlx::query_as(&format!(
        "SELECT {cols} FROM sales_document_assets WHERE document_id = $1"
    ))
    .bind(doc_id)
    .fetch_all(&mut *db)
    .await?)
}

/// `revise(db, doc, user, reason)`: a new revision: the document and its blocks copied, revision + 1; the
/// earlier one superseded with its approval and verification; its quotations revised too (sales revise
/// flow) and the new ones linked. `doc.status` is set to "superseded" in place, as Python mutates it.
pub async fn revise(
    db: &mut PgConnection,
    doc: &mut SalesDocument,
    user: &AuthUser,
    reason: &str,
) -> ApiResult<SalesDocument> {
    Ok(revise_with_changes(db, doc, user, reason).await?.0)
}

/// `revise`, also returning what changed on its quotations (1 Nov 2026): each issued quotation's copy is a
/// draft dated today and repriced (catalog lines at today's price, typed lines kept), one entry per copy:
/// `{quotation, changes}` or `{quotation, error}`.
pub async fn revise_with_changes(
    db: &mut PgConnection,
    doc: &mut SalesDocument,
    user: &AuthUser,
    reason: &str,
) -> ApiResult<(SalesDocument, Vec<Value>)> {
    let reason_s = pystr::strip(reason);
    if reason_s.is_empty() {
        return Err(ApiError::http(
            400,
            "Say why it is revised (it is printed on the cover)",
        ));
    }
    if doc.status == "superseded" {
        return Err(ApiError::http(
            409,
            "This revision is already superseded; revise the latest one",
        ));
    }
    // fix-longapply: a rejected proposal is read-only (reopen it first)
    if doc.status == "rejected" {
        return Err(ApiError::http(
            409,
            "This proposal was rejected: reopen it first",
        ));
    }
    let blocks = blocks_of(db, doc.id).await?;
    let quotes = quotations_of(db, doc.id).await?;
    let group = match &doc.revision_group {
        Some(g) if !g.is_empty() => g.clone(),
        _ => doc.number.clone(),
    };
    let cover = if truthy(&doc.cover) {
        doc.cover.clone()
    } else {
        json!({})
    };
    let new: SalesDocument = sqlx::query_as(&format!(
        "INSERT INTO sales_documents (id, doc_type, number, revision, revision_group, previous_version_id, \
         revision_reason, full_number, title, subtitle, language, deal_id, organization_id, contact_id, status, \
         validity_days, classification, cover, created_by_id, origin, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, \
         $8, $9, $10, $11, $12, $13, $14, 'draft', $15, $16, $17, $18, $19, false) RETURNING {DOC_COLUMNS}"
    ))
    .bind(Uuid::new_v4())
    .bind(&doc.doc_type)
    .bind(&doc.number)
    .bind(doc.revision + 1)
    .bind(&group)
    .bind(doc.id)
    .bind(py_slice(reason_s, 2000))
    .bind(format!("{group} Rev. {}", doc.revision + 1))
    .bind(&doc.title)
    .bind(&doc.subtitle)
    .bind(&doc.language)
    .bind(doc.deal_id)
    .bind(doc.organization_id)
    .bind(doc.contact_id)
    .bind(doc.validity_days)
    .bind(&doc.classification)
    .bind(&cover)
    .bind(user.id)
    .bind(&doc.origin)
    .fetch_one(&mut *db)
    .await?;
    for b in &blocks {
        // json.loads(json.dumps(b.data)): a deep copy, the same JSON.
        insert_block(
            db,
            new.id,
            &Value::String(b.section.clone()),
            b.position,
            &b.block_type,
            &b.data,
        )
        .await?;
    }
    let assets = assets_of(db, doc.id).await?;
    for a in &assets {
        // the same images, kept for both revisions (error and created_by_id are not copied)
        sqlx::query(
            "INSERT INTO sales_document_assets (id, document_id, kind, filename, mime, storage_key, caption, scene, \
             code, model_name, status, checks, attempts, promoted) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(Uuid::new_v4())
        .bind(new.id)
        .bind(&a.kind)
        .bind(&a.filename)
        .bind(&a.mime)
        .bind(&a.storage_key)
        .bind(&a.caption)
        .bind(&a.scene)
        .bind(&a.code)
        .bind(&a.model_name)
        .bind(&a.status)
        .bind(&a.checks)
        .bind(a.attempts)
        .bind(a.promoted)
        .execute(&mut *db)
        .await?;
    }
    // Python quirk kept: both lists are read without ORDER BY and paired by position (zip), which relies on
    // the heap returning rows in insertion order.
    let new_assets = assets_of(db, new.id).await?;
    let remap: Vec<(String, String)> = assets
        .iter()
        .zip(new_assets.iter())
        .map(|(o, n)| (o.id.hyphenated().to_string(), n.id.hyphenated().to_string()))
        .collect();
    for b in blocks_of(db, new.id).await? {
        if super::figures::is_figure(&b.block_type) {
            // visuals V2: a figure points at its SVG and PNG copies
            let Some(obj) = b.data.as_object() else {
                continue;
            };
            let mut data = obj.clone();
            let mut moved = false;
            for k in ["svg_asset_id", "png_asset_id"] {
                let Some(aid) = obj.get(k).and_then(Value::as_str) else {
                    continue;
                };
                if let Some((_, to)) = remap.iter().find(|(from, _)| from == aid) {
                    data.insert(k.into(), Value::String(to.clone()));
                    moved = true;
                }
            }
            if moved {
                sqlx::query(
                    "UPDATE sales_document_blocks SET data = $1, updated_at = now() WHERE id = $2",
                )
                .bind(Value::Object(data))
                .bind(b.id)
                .execute(&mut *db)
                .await?;
            }
            continue;
        }
        if b.block_type != "image" {
            continue;
        }
        let Some(obj) = b.data.as_object() else {
            continue;
        };
        let Some(aid) = obj.get("asset_id").and_then(Value::as_str) else {
            continue;
        };
        if let Some((_, to)) = remap.iter().find(|(from, _)| from == aid) {
            let mut data = obj.clone();
            data.insert("asset_id".into(), Value::String(to.clone()));
            sqlx::query(
                "UPDATE sales_document_blocks SET data = $1, updated_at = now() WHERE id = $2",
            )
            .bind(Value::Object(data))
            .bind(b.id)
            .execute(&mut *db)
            .await?;
        }
    }
    sqlx::query(
        "UPDATE sales_documents SET status = 'superseded', updated_at = now() WHERE id = $1",
    )
    .bind(doc.id)
    .execute(&mut *db)
    .await?;
    doc.status = "superseded".into();
    crate::nary::proposal_lifecycle::close_review(
        db,
        doc.id,
        crate::nary::wa_send::current_card(),
        "the proposal was superseded by a revision.",
        true,
    )
    .await?;
    let short = py_slice(reason_s, 200);
    // and its QR codes
    crate::approvals::engine::supersede_for_entity(
        db,
        "sales_document",
        doc.id,
        &format!("Revised: {short}"),
    )
    .await?;
    let new_full = new.full_number.clone().unwrap_or_default();
    let mut changes: Vec<Value> = vec![];
    for (i, (q, label)) in quotes.iter().enumerate() {
        let status = q.status.as_deref().unwrap_or("");
        // 1 Nov 2026: a cancelled quotation stays with the old revision too
        if status == "superseded" || status == "cancelled" {
            continue; // the position index still counts it (Python's enumerate): gaps are kept
        }
        let mut qid = q.id;
        if matches!(status, "sent" | "expired" | "rejected") {
            // An issued quotation is revised with the proposal (the sales revise flow supersedes it).
            let out = crate::later::documents::routers_sales_revise_quotation(
                &q.id.hyphenated().to_string(),
                &format!("Proposal {new_full}: {short}"),
                user,
                db,
            )
            .await?;
            qid = py_uuid(out["id"].as_str().unwrap_or_default())?;
            // 1 Nov 2026 (Owner decision): the copy is dated today and repriced
            changes.push(crate::nary::quotation_lifecycle::reprice_copy(db, user, qid).await?);
        }
        insert_link(db, new.id, qid, i as i32, label.as_deref()).await?;
    }
    Ok((new, changes))
}

// ------------------------------------------------- 1 Nov 2026: block operations with stable ids

// Direct proposal authoring (an MCP client at act / full power and Nary's chat, Owner decisions 1 Nov 2026): one block
// or one section changes and every other block keeps its id (replace_blocks re-creates them all). Only while the
// proposal is a draft or in review; the generated blocks (the commercial table from the linked quotations, the
// approval stamp) are never written, and a generated section takes written blocks only when it is one of
// proposal.price_text_sections. Figures are guarded and drawn (figures::materialise) and a picture block needs
// its image stored first: an empty image block is never made. None of these commit.

/// The block types nobody writes: rendered from the linked quotations and the approval.
pub const GENERATED_TYPES: [&str; 2] = ["commercial", "stamp"];

fn check_editable(doc: &SalesDocument) -> ApiResult<()> {
    if EDITABLE.contains(&doc.status.as_str()) {
        return Ok(());
    }
    Err(ApiError::http(
        409,
        format!(
            "A {} proposal can't be edited: make a revision first",
            doc.status
        ),
    ))
}

/// proposal.price_text_sections: the sections whose text may state money (the Commercial Summary and Terms).
pub async fn price_sections(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    let v = require_json(db, "proposal.price_text_sections", WHERE).await?;
    match v.as_array() {
        Some(a) if a.iter().all(Value::is_string) => Ok(a
            .iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect()),
        _ => Err(SettingError::new(
            "proposal.price_text_sections",
            "must be a list of section keys",
            WHERE,
        )
        .into()),
    }
}

/// Whether written blocks may go in a section: Err(reason) for a key the template does not have, or a
/// generated section (the commercial table, the stamp) that is not one of proposal.price_text_sections.
pub async fn writable_section(db: &mut PgConnection, key: &str) -> ApiResult<Result<(), String>> {
    let tmpl = template(db).await?;
    let Some(s) = tmpl
        .iter()
        .find(|s| s.get("key").and_then(Value::as_str) == Some(key))
    else {
        let keys: Vec<&str> = tmpl
            .iter()
            .filter_map(|s| s.get("key").and_then(Value::as_str))
            .collect();
        return Ok(Err(format!(
            "section '{key}' is not in the proposal template (sections: {})",
            keys.join(", ")
        )));
    };
    let generated = s
        .get("blocks")
        .and_then(Value::as_array)
        .is_some_and(|b| b.iter().any(|x| x == "commercial" || x == "stamp"));
    if generated && !price_sections(db).await?.iter().any(|p| p == key) {
        return Ok(Err(format!(
            "section '{key}' is generated (the commercial table from the linked quotations, the approval \
             stamp): it can't be written"
        )));
    }
    Ok(Ok(()))
}

async fn block_in(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
) -> ApiResult<SalesDocumentBlock> {
    blocks_of(db, doc.id)
        .await?
        .into_iter()
        .find(|b| b.id == block_id)
        .ok_or_else(|| ApiError::http(404, "That block is not in this proposal"))
}

/// An image block's picture must be stored with this proposal (status ready) before the block exists.
async fn check_picture(db: &mut PgConnection, doc: &SalesDocument, data: &Value) -> ApiResult<()> {
    let id = data
        .get("asset_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok());
    let ok: Option<(String,)> = match id {
        Some(id) => {
            sqlx::query_as(
                "SELECT status FROM sales_document_assets WHERE id = $1 AND document_id = $2 \
                 AND storage_key IS NOT NULL",
            )
            .bind(id)
            .bind(doc.id)
            .fetch_optional(&mut *db)
            .await?
        }
        None => None,
    };
    match ok {
        Some((s,)) if s == "ready" => Ok(()),
        _ => Err(ApiError::http(
            422,
            "An image block needs its picture stored with this proposal first (proposal_add_image): an \
             empty image block is never made",
        )),
    }
}

/// The data a written block is stored with: validated, a figure guarded and drawn, a picture checked.
async fn written_data(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_type: &str,
    data: &Value,
) -> ApiResult<Value> {
    if GENERATED_TYPES.contains(&block_type) {
        return Err(ApiError::http(
            422,
            format!("A {block_type} block is generated: it can't be written"),
        ));
    }
    let data = validate_block(&json!(block_type), data)?;
    if super::figures::is_figure(block_type) {
        return super::figures::materialise(db, doc, block_type, &data).await;
    }
    if block_type == "image" {
        check_picture(db, doc, &data).await?;
    }
    Ok(data)
}

async fn touch(db: &mut PgConnection, doc: &SalesDocument) -> ApiResult<()> {
    sqlx::query("UPDATE sales_documents SET updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// Adds one block: after `after` (in its section), else in `section` at `position` (0-based among the
/// section's blocks; None = its end). The other blocks keep their ids. Returns the new block.
pub async fn add_block(
    db: &mut PgConnection,
    doc: &SalesDocument,
    section: Option<&str>,
    after: Option<Uuid>,
    position: Option<i64>,
    block_type: &str,
    data: &Value,
) -> ApiResult<SalesDocumentBlock> {
    check_editable(doc)?;
    let (section, at) =
        crate::nary::proposal_images::place(db, doc, section, after, position).await?;
    if let Err(why) = writable_section(db, &section).await? {
        return Err(ApiError::http(422, why));
    }
    let data = written_data(db, doc, block_type, data).await?;
    sqlx::query(
        "UPDATE sales_document_blocks SET position = position + 1, updated_at = now() \
         WHERE document_id = $1 AND position >= $2",
    )
    .bind(doc.id)
    .bind(at)
    .execute(&mut *db)
    .await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sales_document_blocks (id, document_id, section, position, block_type, data) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(doc.id)
    .bind(&section)
    .bind(at)
    .bind(block_type)
    .bind(&data)
    .execute(&mut *db)
    .await?;
    touch(db, doc).await?;
    block_in(db, doc, id).await
}

/// Replaces one block's data (same id, same place, same type). Returns the block.
pub async fn set_block(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
    data: &Value,
) -> ApiResult<SalesDocumentBlock> {
    check_editable(doc)?;
    let b = block_in(db, doc, block_id).await?;
    if let Err(why) = writable_section(db, &b.section).await? {
        return Err(ApiError::http(422, why));
    }
    let data = written_data(db, doc, &b.block_type, data).await?;
    sqlx::query("UPDATE sales_document_blocks SET data = $2, updated_at = now() WHERE id = $1")
        .bind(b.id)
        .bind(&data)
        .execute(&mut *db)
        .await?;
    touch(db, doc).await?;
    block_in(db, doc, b.id).await
}

/// Removes one block (the positions after it close up); the generated ones stay. Returns the removed block.
/// A figure's or picture's assets stay (an earlier revision or the verified copy may show them).
pub async fn delete_block(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
) -> ApiResult<SalesDocumentBlock> {
    check_editable(doc)?;
    let b = block_in(db, doc, block_id).await?;
    if GENERATED_TYPES.contains(&b.block_type.as_str()) {
        return Err(ApiError::http(
            422,
            format!("The {} block is generated: it stays", b.block_type),
        ));
    }
    sqlx::query("DELETE FROM sales_document_blocks WHERE id = $1")
        .bind(b.id)
        .execute(&mut *db)
        .await?;
    sqlx::query(
        "UPDATE sales_document_blocks SET position = position - 1, updated_at = now() \
         WHERE document_id = $1 AND position > $2",
    )
    .bind(doc.id)
    .bind(b.position)
    .execute(&mut *db)
    .await?;
    touch(db, doc).await?;
    Ok(b)
}

/// Replaces one section's text, kv, table and coverage blocks (cleaned blocks as the tools take them, see
/// nary::proposal_sections::clean_section_blocks); its figures, pictures and generated blocks stay (a keep
/// entry places one), every other section keeps its blocks and ids. Returns the section's block ids in order.
pub async fn set_section(
    db: &mut PgConnection,
    doc: &SalesDocument,
    section: &str,
    blocks: &[Value],
) -> ApiResult<Vec<Uuid>> {
    check_editable(doc)?;
    if let Err(why) = writable_section(db, section).await? {
        return Err(ApiError::http(422, why));
    }
    crate::nary::proposal_sections::replace_section(db, doc, section, blocks).await?;
    Ok(blocks_of(db, doc.id)
        .await?
        .into_iter()
        .filter(|b| b.section == section)
        .map(|b| b.id)
        .collect())
}

/// `issue_date(db, doc)`: the document's issue date, or today in the company's time zone.
pub async fn issue_date(db: &mut PgConnection, doc: &SalesDocument) -> ApiResult<NaiveDate> {
    match doc.issue_date {
        Some(d) => Ok(d),
        None => Ok(granary_core::clock::today_local(db).await?),
    }
}

#[cfg(test)]
mod tests {
    use super::section_title_text;
    use serde_json::json;

    #[test]
    fn section_titles_are_english_text_and_older_objects_read_their_en() {
        assert_eq!(
            section_title_text(Some(&json!("Executive Summary"))).as_deref(),
            Some("Executive Summary")
        );
        assert_eq!(
            section_title_text(Some(&json!({"en": "Commercial Summary"}))).as_deref(),
            Some("Commercial Summary")
        );
        assert_eq!(section_title_text(Some(&json!({"fr": "Résumé"}))), None);
        assert_eq!(section_title_text(Some(&json!(""))), None);
        assert_eq!(section_title_text(None), None);
    }
}
