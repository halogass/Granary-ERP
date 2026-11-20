//! Port of backend/app/services/approvals/adapters.py.
//!
//! Per-document adapters: how the engine loads a document, what facts it evaluates, and how a
//! decision is written back onto the document's own columns.
//!
//! `document_facts` is the document's own content (what an approver actually approved) and is
//! fingerprinted: editing any of it invalidates the approval. `snapshot` adds derived and reference
//! facts (base-currency conversion, catalog prices, margins, allowances) that the rules evaluate but that do not
//! invalidate an approval when reference data later changes.
//!
//! Port notes:
//! - Python's class per document type is one `DocumentAdapter` value per type (`ADAPTERS`, same order)
//!   and the loaded ORM object is an `Entity` (one variant per type, holding the generated row structs).
//! - Facts are an ordered `serde_json::Map` (preserve_order) holding exactly the Python values: floats are
//!   JSON numbers whose text is Python's `repr(float)` (arbitrary_precision keeps it), ints stay ints, and
//!   JSON columns pass through as read. `granary_compat::pyjson::PyValue::from(&value)` gives back the
//!   Python value, so `json.dumps` of the facts (the fingerprint) is byte-identical.
//! - The hooks (`on_submitted` / `on_decision` / `on_cancelled`) mutate the entity and write the changed
//!   columns at once (SQLAlchemy writes them at the next flush): only columns whose value actually
//!   changed, plus `updated_at = now()` (TimestampMixin onupdate), and nothing when nothing changed, as
//!   the unit of work does.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use granary_compat::pyjson::{self, PyValue};
use granary_compat::text::pystr;
use granary_compat::text::utils::py_uuid;
use granary_core::auth::AuthUser;
use granary_core::clock::today_local;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::money::{dec_to_f64, quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_db::rows::doc_library::{LibraryDocument, LibraryDocumentVersion}; // doclib A1
use granary_db::rows::documents::SalesDocument;
use granary_db::rows::finance::{Expense, Invoice, InvoiceItem};
use granary_db::rows::hr::{HiringRequest, OfferLetter, PayrollRun};
use granary_db::rows::knowledge::KBArticle;
use granary_db::rows::manufacturing::{BOMItem, BOM};
use granary_db::rows::marketing::ContentCalendarItem;
use granary_db::rows::sales::{Contract, Quotation, QuotationItem};
use granary_db::rows::vendor::{
    ContractorRequest, PurchaseOrder, PurchaseOrderItem, VendorInvoice, WorkOrder,
};
use once_cell::sync::Lazy;
use regex::Regex;
use rust_decimal::Decimal;
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgRow;
use sqlx::PgConnection;
use uuid::Uuid;

/// An ordered Python dict of facts (serde_json is built with preserve_order).
pub type Facts = serde_json::Map<String, Value>;

// ─── value helpers (the Python `_f`, `_d`, `_id`) ──────────────────────────────

/// A Python float as a JSON number whose text is `repr(x)`. Non-finite values cannot occur here
/// (every division is guarded); they would be `NaN`/`Infinity` in Python's json.dumps, null here.
fn fl(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    serde_json::from_str::<Number>(&pyjson::py_float_repr(x))
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn flo(x: Option<f64>) -> Value {
    x.map(fl).unwrap_or(Value::Null)
}

/// `_f(v)`: `None if v is None else float(v)`.
fn f_(v: Option<Decimal>) -> Value {
    v.map(|d| fl(dec_to_f64(d))).unwrap_or(Value::Null)
}

fn fd(v: Decimal) -> Value {
    fl(dec_to_f64(v))
}

fn fv(v: Option<Decimal>) -> Option<f64> {
    v.map(dec_to_f64)
}

/// `float(v or 0)`.
fn f0(v: Option<Decimal>) -> f64 {
    v.map(dec_to_f64).unwrap_or(0.0)
}

/// `_d(v)`: a date's isoformat, else the value (None).
fn d_(v: Option<NaiveDate>) -> Value {
    v.map(|d| Value::String(pyjson::date_isoformat(d)))
        .unwrap_or(Value::Null)
}

/// `_id(v)`: `str(v) if v else None` (a UUID is always truthy).
fn id_(v: Option<Uuid>) -> Value {
    v.map(|u| Value::String(u.to_string()))
        .unwrap_or(Value::Null)
}

fn s_(v: Option<&str>) -> Value {
    v.map(|s| Value::String(s.to_string()))
        .unwrap_or(Value::Null)
}

fn i_(v: Option<i32>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// `s[:n]` (code points).
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `id[:8]`.
fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

/// Python truthiness of an optional string.
fn truthy(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

/// `_now()`: `datetime.now(timezone.utc)`, microsecond precision.
fn now_utc() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap_or_else(Utc::now)
}

/// Python `round(x, 2)` for floats: the correctly rounded decimal (half-even on the exact binary value).
fn py_round2(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.2}").parse::<f64>().unwrap_or(x)
}

/// Python `max(iterable, default=0.0)` over floats (the first of equal maxima wins).
fn py_max(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut best: Option<f64> = None;
    for v in values {
        match best {
            None => best = Some(v),
            Some(b) if v > b => best = Some(v),
            _ => {}
        }
    }
    best.unwrap_or(0.0)
}

fn internal(msg: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

// ─── writing changed columns (SQLAlchemy's flush of a dirty object) ────────────

/// The columns a hook changed on one row, with their new values.
#[derive(Default)]
struct Dirty(Vec<(&'static str, Value)>);

/// `row.field = value`, recorded only when the value actually changes (SQLAlchemy emits no UPDATE for a
/// value equal to the loaded one).
macro_rules! set {
    ($dirty:expr, $row:expr, $field:ident, $val:expr) => {{
        let v = $val;
        if $row.$field != v {
            $row.$field = v;
            $dirty.0.push((
                stringify!($field),
                serde_json::to_value(&$row.$field).unwrap_or(Value::Null),
            ));
        }
    }};
}

/// UPDATE the changed columns plus `updated_at = now()`; nothing when nothing changed.
async fn flush(db: &mut PgConnection, table: &str, id: Uuid, dirty: Dirty) -> ApiResult<()> {
    if dirty.0.is_empty() {
        return Ok(());
    }
    let mut obj = serde_json::Map::new();
    for (col, val) in dirty.0 {
        obj.insert(col.to_string(), val);
    }
    let sets = obj
        .keys()
        .map(|c| format!("\"{c}\" = r.\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query(&format!(
        "UPDATE {table} AS t SET {sets}, updated_at = now() \
         FROM jsonb_populate_record(NULL::{table}, $1) AS r WHERE t.id = $2"
    ))
    .bind(Value::Object(obj))
    .bind(id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

// ─── loaded documents ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct QuotationDoc {
    pub row: Quotation,
    /// `Quotation.items` (selectinload, order_by sort_order).
    pub items: Vec<QuotationItem>,
    /// `q.customer_org.name` (None without an organization).
    pub customer_org_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PurchaseOrderDoc {
    pub row: PurchaseOrder,
    /// `PurchaseOrder.items` (selectinload, no order_by: heap order).
    pub items: Vec<PurchaseOrderItem>,
}

#[derive(Debug, Clone)]
pub struct BomDoc {
    pub row: BOM,
    /// `BOM.items` (selectinload, no order_by: heap order).
    pub items: Vec<BOMItem>,
    /// `b.product.name` (None when the product row is gone).
    pub product_name: Option<String>,
}

/// doclib A1: a library document version waiting for a director (its document alongside).
#[derive(Debug, Clone)]
pub struct LibraryVersionDoc {
    pub row: LibraryDocumentVersion,
    pub document: LibraryDocument,
}

/// A loaded document, one variant per adapter.
#[derive(Debug, Clone)]
pub enum Entity {
    Quotation(Box<QuotationDoc>),
    SalesContract(Box<Contract>),
    Invoice(Box<Invoice>),
    Expense(Box<Expense>),
    PurchaseOrder(Box<PurchaseOrderDoc>),
    ContractorRequest(Box<ContractorRequest>),
    WorkOrder(Box<WorkOrder>),
    VendorInvoice(Box<VendorInvoice>),
    MarketingContent(Box<ContentCalendarItem>),
    HiringRequest(Box<HiringRequest>),
    OfferLetter(Box<OfferLetter>),
    SopArticle(Box<KBArticle>),
    PayrollRun(Box<PayrollRun>),
    SalesDocument(Box<SalesDocument>),
    Bom(Box<BomDoc>),
    LibraryDocumentVersion(Box<LibraryVersionDoc>), // doclib A1
}

impl Entity {
    pub fn id(&self) -> Uuid {
        match self {
            Entity::Quotation(q) => q.row.id,
            Entity::SalesContract(c) => c.id,
            Entity::Invoice(i) => i.id,
            Entity::Expense(e) => e.id,
            Entity::PurchaseOrder(p) => p.row.id,
            Entity::ContractorRequest(r) => r.id,
            Entity::WorkOrder(w) => w.id,
            Entity::VendorInvoice(v) => v.id,
            Entity::MarketingContent(c) => c.id,
            Entity::HiringRequest(h) => h.id,
            Entity::OfferLetter(o) => o.id,
            Entity::SopArticle(a) => a.id,
            Entity::PayrollRun(p) => p.id,
            Entity::SalesDocument(d) => d.id,
            Entity::Bom(b) => b.row.id,
            Entity::LibraryDocumentVersion(v) => v.row.id,
        }
    }

    /// `getattr(entity, "status", None)`.
    pub fn status(&self) -> Option<&str> {
        match self {
            Entity::Quotation(q) => q.row.status.as_deref(),
            Entity::SalesContract(c) => c.status.as_deref(),
            Entity::Invoice(i) => i.status.as_deref(),
            Entity::Expense(e) => e.status.as_deref(),
            Entity::PurchaseOrder(p) => p.row.status.as_deref(),
            Entity::ContractorRequest(r) => r.status.as_deref(),
            Entity::WorkOrder(w) => w.status.as_deref(),
            Entity::VendorInvoice(v) => v.status.as_deref(),
            Entity::MarketingContent(c) => c.status.as_deref(),
            Entity::HiringRequest(h) => h.status.as_deref(),
            Entity::OfferLetter(o) => o.status.as_deref(),
            Entity::SopArticle(a) => a.status.as_deref(),
            Entity::PayrollRun(p) => p.status.as_deref(),
            Entity::SalesDocument(d) => Some(d.status.as_str()),
            Entity::Bom(b) => b.row.status.as_deref(),
            Entity::LibraryDocumentVersion(v) => Some(v.row.status.as_str()),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Entity::Quotation(_) => Kind::Quotation,
            Entity::SalesContract(_) => Kind::SalesContract,
            Entity::Invoice(_) => Kind::Invoice,
            Entity::Expense(_) => Kind::Expense,
            Entity::PurchaseOrder(_) => Kind::PurchaseOrder,
            Entity::ContractorRequest(_) => Kind::ContractorRequest,
            Entity::WorkOrder(_) => Kind::WorkOrder,
            Entity::VendorInvoice(_) => Kind::VendorInvoice,
            Entity::MarketingContent(_) => Kind::MarketingContent,
            Entity::HiringRequest(_) => Kind::HiringRequest,
            Entity::OfferLetter(_) => Kind::OfferLetter,
            Entity::SopArticle(_) => Kind::SopArticle,
            Entity::PayrollRun(_) => Kind::PayrollRun,
            Entity::SalesDocument(_) => Kind::SalesDocument,
            Entity::Bom(_) => Kind::Bom,
            Entity::LibraryDocumentVersion(_) => Kind::LibraryDocumentVersion,
        }
    }

    pub fn entity_type(&self) -> &'static str {
        self.kind().entity_type()
    }

    /// `getattr(entity, "number", None)` for the models that have a `number` column.
    fn number_attr(&self) -> Option<&str> {
        match self {
            Entity::Quotation(q) => Some(q.row.number.as_str()),
            Entity::SalesContract(c) => Some(c.number.as_str()),
            Entity::Invoice(i) => Some(i.number.as_str()),
            Entity::PurchaseOrder(p) => Some(p.row.number.as_str()),
            Entity::WorkOrder(w) => Some(w.number.as_str()),
            Entity::OfferLetter(o) => Some(o.number.as_str()),
            Entity::SalesDocument(d) => Some(d.number.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Quotation,
    SalesContract,
    Invoice,
    Expense,
    PurchaseOrder,
    ContractorRequest,
    WorkOrder,
    VendorInvoice,
    MarketingContent,
    HiringRequest,
    OfferLetter,
    SopArticle,
    PayrollRun,
    SalesDocument,
    Bom,
    LibraryDocumentVersion, // doclib A1
}

impl Kind {
    pub fn entity_type(self) -> &'static str {
        ADAPTERS
            .iter()
            .find(|a| a.kind == self)
            .map(|a| a.entity_type)
            .unwrap_or("")
    }
}

/// `DocumentAdapter` and its subclasses (one value per entity type).
#[derive(Debug)]
pub struct DocumentAdapter {
    pub kind: Kind,
    pub entity_type: &'static str,
    pub label: &'static str,
    pub link_template: &'static str,
    pub submittable_statuses: Option<&'static [&'static str]>,
}

/// `ADAPTERS`, in the Python dict's order.
pub static ADAPTERS: [DocumentAdapter; 16] = [
    DocumentAdapter {
        kind: Kind::Quotation,
        entity_type: "quotation",
        label: "Quotation",
        link_template: "/sales/quotations/{id}",
        submittable_statuses: Some(&["draft"]),
    },
    DocumentAdapter {
        kind: Kind::SalesContract,
        entity_type: "sales_contract",
        label: "Contract",
        link_template: "/sales/contracts?contract={id}",
        submittable_statuses: Some(&["draft"]),
    },
    DocumentAdapter {
        kind: Kind::Invoice,
        entity_type: "invoice",
        label: "Invoice",
        link_template: "/finance/invoices/{id}",
        submittable_statuses: Some(&["draft"]),
    },
    DocumentAdapter {
        kind: Kind::Expense,
        entity_type: "expense",
        label: "Expense claim",
        link_template: "/finance/expenses?expense={id}",
        submittable_statuses: Some(&["pending", "rejected"]),
    },
    DocumentAdapter {
        kind: Kind::PurchaseOrder,
        entity_type: "purchase_order",
        label: "Purchase order",
        link_template: "/vendor-mgmt/purchase-orders?po={id}",
        submittable_statuses: Some(&["draft"]),
    },
    DocumentAdapter {
        kind: Kind::ContractorRequest,
        entity_type: "contractor_request",
        label: "Contractor request",
        link_template: "/vendor-mgmt/contractor-requests?request={id}",
        submittable_statuses: Some(&["pending_approval", "rejected"]),
    },
    DocumentAdapter {
        kind: Kind::WorkOrder,
        entity_type: "work_order",
        label: "Work order",
        link_template: "/vendor-mgmt/work-orders?wo={id}",
        submittable_statuses: Some(&["draft"]),
    },
    DocumentAdapter {
        kind: Kind::VendorInvoice,
        entity_type: "vendor_invoice",
        label: "Vendor invoice",
        link_template: "/vendor-mgmt/invoices?invoice={id}",
        submittable_statuses: Some(&["submitted", "rejected"]),
    },
    DocumentAdapter {
        kind: Kind::MarketingContent,
        entity_type: "marketing_content",
        label: "Content",
        link_template: "/marketing/content?item={id}",
        submittable_statuses: Some(&["planned", "in_progress", "review"]),
    },
    DocumentAdapter {
        kind: Kind::HiringRequest,
        entity_type: "hiring_request",
        label: "Headcount request",
        link_template: "/hr/hiring?request={id}",
        submittable_statuses: Some(&["pending_approval", "on_hold"]),
    },
    DocumentAdapter {
        kind: Kind::OfferLetter,
        entity_type: "offer_letter",
        label: "Offer letter",
        link_template: "/hr/offer-letters/{id}",
        submittable_statuses: Some(&["draft"]),
    },
    DocumentAdapter {
        kind: Kind::SopArticle,
        entity_type: "sop_article",
        label: "SOP",
        link_template: "/knowledge/articles/{id}",
        submittable_statuses: Some(&["draft", "review"]),
    },
    DocumentAdapter {
        kind: Kind::PayrollRun,
        entity_type: "payroll_run",
        label: "Payroll run",
        link_template: "/hr/payroll?run={id}",
        submittable_statuses: Some(&["draft"]),
    },
    DocumentAdapter {
        kind: Kind::SalesDocument,
        entity_type: "sales_document",
        label: "Solution Proposal",
        link_template: "/sales/proposals/{id}",
        submittable_statuses: Some(&["draft", "review"]),
    },
    DocumentAdapter {
        kind: Kind::Bom,
        entity_type: "bom",
        label: "Bill of materials",
        link_template: "/manufacturing/products?bom={id}",
        submittable_statuses: Some(&["draft"]),
    },
    // doclib A1: a new version of a customer/partner document in the library (the page resolves the version)
    DocumentAdapter {
        kind: Kind::LibraryDocumentVersion,
        entity_type: "library_document_version",
        label: "Document version",
        link_template: "/knowledge/documents?version={id}",
        submittable_statuses: Some(&["draft", "pending_approval"]),
    },
];

/// `get_adapter(entity_type)`.
pub fn get_adapter(entity_type: &str) -> Result<&'static DocumentAdapter, ApiError> {
    ADAPTERS
        .iter()
        .find(|a| a.entity_type == entity_type)
        .ok_or_else(|| {
            ApiError::http(
                400,
                format!("Unknown document type: {}", pyjson::repr_str(entity_type)),
            )
        })
}

// ─── loading ──────────────────────────────────────────────────────────────────

async fn fetch_row<T>(
    db: &mut PgConnection,
    table: &str,
    columns: &[&str],
    uid: Uuid,
    soft: bool,
) -> ApiResult<Option<T>>
where
    T: for<'r> sqlx::FromRow<'r, PgRow> + Send + Unpin,
{
    let cols = columns.join(", ");
    let deleted = if soft { " AND is_deleted = false" } else { "" };
    Ok(sqlx::query_as::<_, T>(&format!(
        "SELECT {cols} FROM {table} WHERE id = $1{deleted}"
    ))
    .bind(uid)
    .fetch_optional(&mut *db)
    .await?)
}

async fn fetch_children<T>(
    db: &mut PgConnection,
    table: &str,
    columns: &[&str],
    fk: &str,
    uid: Uuid,
    order: &str,
) -> ApiResult<Vec<T>>
where
    T: for<'r> sqlx::FromRow<'r, PgRow> + Send + Unpin,
{
    let cols = columns.join(", ");
    Ok(sqlx::query_as::<_, T>(&format!(
        "SELECT {cols} FROM {table} WHERE {fk} IN ($1){order}"
    ))
    .bind(uid)
    .fetch_all(&mut *db)
    .await?)
}

async fn name_of(
    db: &mut PgConnection,
    table: &str,
    id: Option<Uuid>,
) -> ApiResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    let row: Option<(String,)> = sqlx::query_as(&format!("SELECT name FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(row.map(|r| r.0))
}

fn boxed<T>(v: Option<T>, f: impl FnOnce(Box<T>) -> Entity) -> Option<Entity> {
    v.map(|x| f(Box::new(x)))
}

impl DocumentAdapter {
    /// `load(db, entity_id)`: None for an id that is not a UUID (Python `uuid.UUID(str(...))` rules) or a
    /// missing / soft-deleted row.
    pub async fn load(
        &self,
        db: &mut PgConnection,
        entity_id: &str,
    ) -> Result<Option<Entity>, ApiError> {
        let Some(uid) = py_uuid(entity_id) else {
            return Ok(None);
        };
        Ok(match self.kind {
            Kind::Quotation => {
                let Some(row) =
                    fetch_row::<Quotation>(db, Quotation::TABLE, Quotation::COLUMNS, uid, true)
                        .await?
                else {
                    return Ok(None);
                };
                let items = fetch_children::<QuotationItem>(
                    db,
                    QuotationItem::TABLE,
                    QuotationItem::COLUMNS,
                    "quotation_id",
                    uid,
                    " ORDER BY sort_order",
                )
                .await?;
                let customer_org_name = name_of(db, "organizations", row.customer_org_id).await?;
                Some(Entity::Quotation(Box::new(QuotationDoc {
                    row,
                    items,
                    customer_org_name,
                })))
            }
            Kind::SalesContract => boxed(
                fetch_row::<Contract>(db, Contract::TABLE, Contract::COLUMNS, uid, true).await?,
                Entity::SalesContract,
            ),
            Kind::Invoice => boxed(
                fetch_row::<Invoice>(db, Invoice::TABLE, Invoice::COLUMNS, uid, true).await?,
                Entity::Invoice,
            ),
            Kind::Expense => boxed(
                fetch_row::<Expense>(db, Expense::TABLE, Expense::COLUMNS, uid, true).await?,
                Entity::Expense,
            ),
            Kind::PurchaseOrder => {
                let Some(row) = fetch_row::<PurchaseOrder>(
                    db,
                    PurchaseOrder::TABLE,
                    PurchaseOrder::COLUMNS,
                    uid,
                    true,
                )
                .await?
                else {
                    return Ok(None);
                };
                let items = fetch_children::<PurchaseOrderItem>(
                    db,
                    PurchaseOrderItem::TABLE,
                    PurchaseOrderItem::COLUMNS,
                    "po_id",
                    uid,
                    "",
                )
                .await?;
                Some(Entity::PurchaseOrder(Box::new(PurchaseOrderDoc {
                    row,
                    items,
                })))
            }
            Kind::ContractorRequest => boxed(
                fetch_row::<ContractorRequest>(
                    db,
                    ContractorRequest::TABLE,
                    ContractorRequest::COLUMNS,
                    uid,
                    true,
                )
                .await?,
                Entity::ContractorRequest,
            ),
            Kind::WorkOrder => boxed(
                fetch_row::<WorkOrder>(db, WorkOrder::TABLE, WorkOrder::COLUMNS, uid, true).await?,
                Entity::WorkOrder,
            ),
            Kind::VendorInvoice => boxed(
                fetch_row::<VendorInvoice>(
                    db,
                    VendorInvoice::TABLE,
                    VendorInvoice::COLUMNS,
                    uid,
                    true,
                )
                .await?,
                Entity::VendorInvoice,
            ),
            Kind::MarketingContent => boxed(
                fetch_row::<ContentCalendarItem>(
                    db,
                    ContentCalendarItem::TABLE,
                    ContentCalendarItem::COLUMNS,
                    uid,
                    true,
                )
                .await?,
                Entity::MarketingContent,
            ),
            Kind::HiringRequest => boxed(
                fetch_row::<HiringRequest>(
                    db,
                    HiringRequest::TABLE,
                    HiringRequest::COLUMNS,
                    uid,
                    true,
                )
                .await?,
                Entity::HiringRequest,
            ),
            Kind::OfferLetter => boxed(
                fetch_row::<OfferLetter>(db, OfferLetter::TABLE, OfferLetter::COLUMNS, uid, true)
                    .await?,
                Entity::OfferLetter,
            ),
            Kind::SopArticle => boxed(
                fetch_row::<KBArticle>(db, KBArticle::TABLE, KBArticle::COLUMNS, uid, true).await?,
                Entity::SopArticle,
            ),
            // PayrollRun has no is_deleted column.
            Kind::PayrollRun => boxed(
                fetch_row::<PayrollRun>(db, PayrollRun::TABLE, PayrollRun::COLUMNS, uid, false)
                    .await?,
                Entity::PayrollRun,
            ),
            Kind::SalesDocument => boxed(
                fetch_row::<SalesDocument>(
                    db,
                    SalesDocument::TABLE,
                    SalesDocument::COLUMNS,
                    uid,
                    true,
                )
                .await?,
                Entity::SalesDocument,
            ),
            Kind::Bom => {
                let Some(row) = fetch_row::<BOM>(db, BOM::TABLE, BOM::COLUMNS, uid, true).await?
                else {
                    return Ok(None);
                };
                let items = fetch_children::<BOMItem>(
                    db,
                    BOMItem::TABLE,
                    BOMItem::COLUMNS,
                    "bom_id",
                    uid,
                    "",
                )
                .await?;
                let product_name = name_of(db, "hardware_products", Some(row.product_id)).await?;
                Some(Entity::Bom(Box::new(BomDoc {
                    row,
                    items,
                    product_name,
                })))
            }
            Kind::LibraryDocumentVersion => {
                let Some(row) = fetch_row::<LibraryDocumentVersion>(
                    db,
                    LibraryDocumentVersion::TABLE,
                    LibraryDocumentVersion::COLUMNS,
                    uid,
                    false,
                )
                .await?
                else {
                    return Ok(None);
                };
                let Some(document) = fetch_row::<LibraryDocument>(
                    db,
                    LibraryDocument::TABLE,
                    LibraryDocument::COLUMNS,
                    row.document_id,
                    true,
                )
                .await?
                else {
                    return Ok(None);
                };
                Some(Entity::LibraryDocumentVersion(Box::new(
                    LibraryVersionDoc { row, document },
                )))
            }
        })
    }

    // ─── naming ────────────────────────────────────────────────────────────

    /// `number(entity)`.
    pub fn number(&self, e: &Entity) -> String {
        match e {
            Entity::Quotation(q) => truthy(q.row.full_number.as_deref())
                .unwrap_or(&q.row.number)
                .to_string(),
            Entity::Invoice(i) => truthy(i.full_number.as_deref())
                .unwrap_or(&i.number)
                .to_string(),
            Entity::SalesDocument(d) => truthy(d.full_number.as_deref())
                .unwrap_or(&d.number)
                .to_string(),
            Entity::Bom(b) => format!("v{}", b.row.version),
            Entity::LibraryDocumentVersion(v) => v.row.label.clone(),
            Entity::VendorInvoice(vi) => truthy(vi.vendor_invoice_number.as_deref())
                .map(str::to_string)
                .unwrap_or_else(|| short_id(vi.id)),
            Entity::SopArticle(a) => truthy(a.sop_code.as_deref())
                .or(truthy(Some(a.slug.as_str())))
                .map(str::to_string)
                .unwrap_or_else(|| short_id(a.id)),
            other => truthy(other.number_attr())
                .map(str::to_string)
                .unwrap_or_else(|| short_id(other.id())),
        }
    }

    /// `document_label(entity)`.
    pub fn document_label(&self, e: &Entity) -> String {
        match e {
            Entity::Quotation(q) => {
                let mut s = format!("Quotation {}", self.number(e));
                if let Some(c) = truthy(q.customer_org_name.as_deref()) {
                    s.push_str(&format!(" for {c}"));
                }
                s
            }
            Entity::Invoice(inv) => {
                let kind = match inv.invoice_type.as_deref() {
                    Some("credit_note") => "Credit note",
                    Some("proforma") => "Proforma invoice",
                    _ => "Invoice",
                };
                let mut s = format!("{kind} {}", self.number(e));
                if let Some(c) = truthy(inv.customer_name.as_deref()) {
                    s.push_str(&format!(" for {c}"));
                }
                s
            }
            Entity::Expense(x) => format!(
                "Expense claim ({}): {}",
                x.category,
                head(&x.description, 80)
            ),
            Entity::Bom(b) => {
                let mut s = format!("BOM {}", self.number(e));
                // getattr(b, "product", None): the relationship object is truthy whenever it exists.
                if let Some(name) = &b.product_name {
                    s.push_str(&format!(" for {name}"));
                }
                s
            }
            Entity::ContractorRequest(r) => format!(
                "Contractor request ({}): {}",
                truthy(r.department.as_deref()).unwrap_or("n/a"),
                head(&r.scope_description, 80)
            ),
            Entity::MarketingContent(c) => format!("Content: {}", c.title),
            Entity::HiringRequest(h) => {
                format!("Headcount request: {} ({})", h.title, h.department)
            }
            Entity::OfferLetter(o) => {
                format!("Offer letter {}: {}", o.number, o.position_title)
            }
            Entity::SopArticle(a) => format!("SOP {}: {}", self.number(e), a.title),
            Entity::PayrollRun(p) => format!("Payroll {:02}/{}", p.month, p.year),
            Entity::SalesDocument(d) => {
                format!("Solution Proposal {}: {}", self.number(e), d.title)
            }
            Entity::LibraryDocumentVersion(v) => {
                format!("Document {} {}", v.document.title, v.row.label)
            }
            _ => format!("{} {}", self.label, self.number(e)),
        }
    }

    /// `revision(entity)`.
    pub fn revision(&self, e: &Entity) -> i32 {
        let r = match e {
            Entity::Quotation(q) => q.row.revision,
            Entity::Invoice(i) => i.revision,
            Entity::SalesDocument(d) => Some(d.revision),
            _ => return 1,
        };
        match r {
            Some(v) if v != 0 => v,
            _ => 1,
        }
    }

    /// `expiry_date(entity)`: the date the document itself lapses (its QR link expires the day after);
    /// None = policy TTL.
    pub fn expiry_date(&self, e: &Entity) -> Option<NaiveDate> {
        match e {
            Entity::Quotation(q) => Some(q.row.valid_until),
            Entity::SalesContract(c) => c.end_date,
            Entity::OfferLetter(o) => o.acceptance_deadline,
            Entity::SalesDocument(d) => match d.issue_date {
                Some(issue) if d.validity_days != 0 => {
                    issue.checked_add_signed(Duration::days(d.validity_days as i64))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// `link_for_id(entity_id)`.
    pub fn link_for_id(&self, entity_id: &str) -> String {
        self.link_template.replace("{id}", entity_id)
    }

    /// `check_submittable(entity)`.
    pub fn check_submittable(&self, e: &Entity) -> Result<(), ApiError> {
        match e {
            Entity::PurchaseOrder(po) => {
                // A draft, or a PO already sent without an approval (sent before the policy existed):
                // receiving it then needs the approval first (routers/p2p.py create_receipt).
                let status = po.row.status.as_deref();
                if status == Some("draft")
                    || (matches!(status, Some("sent" | "confirmed" | "partially_received"))
                        && po.row.approval_status.as_deref() != Some("approved"))
                {
                    return Ok(());
                }
                Err(ApiError::http(
                    400,
                    format!(
                        "A purchase order with status '{}' cannot be submitted for approval",
                        status.unwrap_or("None")
                    ),
                ))
            }
            Entity::SopArticle(a) if a.article_type.as_deref() != Some("sop") => Err(
                ApiError::http(400, "Only SOP articles go through publication approval"),
            ),
            _ => {
                let status = e.status();
                if let Some(allowed) = self.submittable_statuses {
                    if !status.is_some_and(|s| allowed.contains(&s)) {
                        return Err(ApiError::http(
                            400,
                            format!(
                                "A {} with status '{}' cannot be submitted for approval",
                                self.label.to_lowercase(),
                                status.unwrap_or("None")
                            ),
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    // ─── facts ─────────────────────────────────────────────────────────────

    /// `document_facts(db, entity)`.
    pub async fn document_facts(
        &self,
        db: &mut PgConnection,
        e: &Entity,
    ) -> Result<Facts, ApiError> {
        let mut m = Facts::new();
        let mut put = |k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        match e {
            Entity::Quotation(qd) => {
                let q = &qd.row;
                put("number", Value::String(self.number(e)));
                put("revision", i_(q.revision));
                put("currency", s_(q.currency.as_deref()));
                put("subtotal", f_(q.subtotal));
                put("total", f_(q.total));
                put("header_discount_type", s_(q.discount_type.as_deref()));
                put("header_discount_value", f_(q.discount_value));
                put(
                    "payment_terms",
                    q.payment_terms.clone().unwrap_or(Value::Null),
                );
                put("payment_terms_days", i_(q.payment_terms_days));
                put(
                    "procurement_type",
                    Value::String(
                        truthy(q.procurement_type.as_deref())
                            .unwrap_or("standard")
                            .to_string(),
                    ),
                );
                put("customer_org_id", id_(q.customer_org_id));
                put("valid_until", d_(Some(q.valid_until)));
                // 17 Nov 2026 (Owner): the printed notes carry payment and legal terms, so an approval covers them;
                // editing them needs approval again (approvals made before this keep their own facts: as_approved)
                // (only when there are notes: a quotation without notes has the facts it always had)
                if let Some(n) = q.notes.as_deref().filter(|n| !n.trim().is_empty()) {
                    put("notes", Value::String(n.to_string()));
                }
                put(
                    "lines",
                    Value::Array(
                        qd.items
                            .iter()
                            .map(|i| Value::Object(quotation_item_facts(i)))
                            .collect(),
                    ),
                );
            }
            Entity::SalesContract(c) => {
                put("number", Value::String(c.number.clone()));
                put("contract_type", s_(c.contract_type.as_deref()));
                put("value", f_(c.value));
                put("currency", s_(c.currency.as_deref()));
                put("start_date", d_(c.start_date));
                put("end_date", d_(c.end_date));
                put("customer_org_id", id_(c.customer_org_id));
                put("quotation_id", id_(c.quotation_id));
                put("document_url", s_(c.document_url.as_deref()));
                put("auto_renew", Value::Bool(c.auto_renew.unwrap_or(false)));
            }
            Entity::Invoice(inv) => {
                let items: Vec<InvoiceItem> = sqlx::query_as(&format!(
                    "SELECT {} FROM invoice_items WHERE invoice_id = $1 ORDER BY sort_order",
                    InvoiceItem::COLUMNS.join(", ")
                ))
                .bind(inv.id)
                .fetch_all(&mut *db)
                .await?;
                put("number", Value::String(self.number(e)));
                put("invoice_type", s_(inv.invoice_type.as_deref()));
                put("currency", s_(inv.currency.as_deref()));
                put("subtotal", f_(inv.subtotal));
                put("tax_total", f_(inv.tax_total));
                put("total", f_(inv.total));
                put("issue_date", d_(Some(inv.issue_date)));
                put("due_date", d_(inv.due_date));
                put("payment_terms_text", s_(inv.payment_terms_text.as_deref()));
                put("customer_org_id", id_(inv.customer_org_id));
                put("customer_name", s_(inv.customer_name.as_deref()));
                put("quotation_id", id_(inv.quotation_id));
                put("credit_of_invoice_id", id_(inv.credit_of_invoice_id));
                put(
                    "items",
                    Value::Array(
                        items
                            .iter()
                            .map(|i| Value::Object(invoice_item_facts(i)))
                            .collect(),
                    ),
                );
            }
            Entity::Expense(x) => {
                put("category", Value::String(x.category.clone()));
                put("description", Value::String(x.description.clone()));
                put("amount", fd(x.amount));
                put("currency", s_(x.currency.as_deref()));
                put("expense_date", d_(Some(x.expense_date)));
                put("receipt_url", s_(x.receipt_url.as_deref()));
                put("project_id", id_(x.project_id));
                put("vendor_id", id_(x.vendor_id));
            }
            Entity::PurchaseOrder(pd) => {
                let po = &pd.row;
                put("number", Value::String(po.number.clone()));
                put("vendor_id", id_(Some(po.vendor_id)));
                put("currency", s_(po.currency.as_deref()));
                put("total", f_(po.total));
                put("expected_delivery", d_(po.expected_delivery));
                put(
                    "items",
                    Value::Array(
                        pd.items
                            .iter()
                            .map(|i| {
                                let mut f = Facts::new();
                                f.insert(
                                    "description".into(),
                                    Value::String(i.description.clone()),
                                );
                                f.insert("quantity".into(), fd(i.quantity_ordered));
                                f.insert("unit_price".into(), fd(i.unit_price));
                                f.insert("amount".into(), fd(i.amount));
                                Value::Object(f)
                            })
                            .collect(),
                    ),
                );
            }
            Entity::Bom(bd) => {
                let b = &bd.row;
                put("product_id", Value::String(b.product_id.to_string()));
                put("version", Value::String(b.version.clone()));
                put("notes", s_(b.notes.as_deref()));
                put(
                    "items",
                    Value::Array(
                        bd.items
                            .iter()
                            .map(|i| {
                                let mut f = Facts::new();
                                f.insert(
                                    "component".into(),
                                    Value::String(i.component_name.clone()),
                                );
                                f.insert("sku".into(), s_(i.component_sku.as_deref()));
                                f.insert("quantity".into(), fd(i.quantity));
                                f.insert("unit_cost".into(), f_(i.unit_cost));
                                f.insert("currency".into(), s_(i.cost_currency.as_deref()));
                                Value::Object(f)
                            })
                            .collect(),
                    ),
                );
            }
            Entity::ContractorRequest(r) => {
                put("department", s_(r.department.as_deref()));
                put("vendor_type", s_(r.vendor_type.as_deref()));
                put(
                    "scope_description",
                    Value::String(r.scope_description.clone()),
                );
                put("estimated_value", f_(r.estimated_value));
                put("currency", s_(r.currency.as_deref()));
                put("needed_by", d_(r.needed_by));
            }
            Entity::WorkOrder(wo) => {
                put("number", Value::String(wo.number.clone()));
                put("vendor_id", id_(Some(wo.vendor_id)));
                put("title", Value::String(wo.title.clone()));
                put("scope", s_(wo.scope.as_deref()));
                put("agreed_value", f_(wo.agreed_value));
                put("agreed_rate", f_(wo.agreed_rate));
                put("currency", s_(wo.currency.as_deref()));
                put("start_date", d_(wo.start_date));
                put("end_date", d_(wo.end_date));
                put("contractor_request_id", id_(wo.contractor_request_id));
            }
            Entity::VendorInvoice(vi) => {
                put("vendor_id", id_(Some(vi.vendor_id)));
                put(
                    "vendor_invoice_number",
                    s_(vi.vendor_invoice_number.as_deref()),
                );
                put("invoice_date", d_(Some(vi.invoice_date)));
                put("amount", fd(vi.amount));
                put("currency", s_(vi.currency.as_deref()));
                put("work_order_id", id_(vi.work_order_id));
                put("purchase_order_id", id_(vi.purchase_order_id));
            }
            Entity::MarketingContent(c) => {
                let body_hash = sha256_hex(c.content_body.as_deref().unwrap_or(""));
                put("title", Value::String(c.title.clone()));
                put("content_type", s_(c.content_type.as_deref()));
                put("platform", s_(c.platform.as_deref()));
                put("planned_date", d_(c.planned_date));
                put("content_body_sha256", Value::String(body_hash));
                put("campaign_id", id_(c.campaign_id));
            }
            Entity::HiringRequest(h) => {
                put("title", Value::String(h.title.clone()));
                put("department", Value::String(h.department.clone()));
                put("employment_type", s_(h.employment_type.as_deref()));
                put("headcount", i_(h.headcount));
                put("urgency", s_(h.urgency.as_deref()));
                put("notes", s_(h.notes.as_deref()));
                put(
                    "budget_approved",
                    Value::Bool(h.budget_approved.unwrap_or(false)),
                );
            }
            Entity::OfferLetter(o) => {
                put("number", Value::String(o.number.clone()));
                put("applicant_id", id_(Some(o.applicant_id)));
                put("position_title", Value::String(o.position_title.clone()));
                put("department", s_(o.department.as_deref()));
                put("employment_type", s_(o.employment_type.as_deref()));
                put("salary_gross_monthly", fd(o.salary_gross_monthly));
                put("salary_currency", s_(o.salary_currency.as_deref()));
                put("start_date", d_(o.start_date));
                put("probation_months", i_(o.probation_months));
                put("benefits", s_(o.benefits.as_deref()));
                put("other_terms", s_(o.other_terms.as_deref()));
                put("acceptance_deadline", d_(o.acceptance_deadline));
                put("reports_to_employee_id", id_(o.reports_to_employee_id));
            }
            Entity::SopArticle(a) => {
                // repr(a.sop_steps): the JSONB value as json.loads gives it (None when NULL).
                let steps = a
                    .sop_steps
                    .as_ref()
                    .map(PyValue::from)
                    .unwrap_or(PyValue::None);
                let steps_repr = pyjson::repr(&steps)
                    .map_err(|e| internal(format!("repr(sop_steps): {e:?}")))?;
                put("title", Value::String(a.title.clone()));
                put("sop_code", s_(a.sop_code.as_deref()));
                put("version", s_(a.version.as_deref()));
                put(
                    "content_sha256",
                    Value::String(sha256_hex(a.content.as_deref().unwrap_or(""))),
                );
                put("sop_steps_sha256", Value::String(sha256_hex(&steps_repr)));
            }
            Entity::PayrollRun(p) => {
                put("month", Value::from(p.month));
                put("year", Value::from(p.year));
                put("currency", s_(p.currency.as_deref()));
                put("total_gross", f_(p.total_gross));
                put("total_net", f_(p.total_net));
                put("total_income_tax", f_(p.total_income_tax));
                put("total_contributions", f_(p.total_contributions));
            }
            Entity::SalesDocument(d) => {
                let quotes = crate::documents::core_::quotations_of(db, d.id).await?;
                let content = crate::documents::core_::fingerprint(db, d).await?;
                put("number", Value::String(self.number(e)));
                put("title", Value::String(d.title.clone()));
                put("language", Value::String(d.language.clone()));
                put("deal_id", id_(d.deal_id));
                put(
                    "quotations",
                    Value::Array(
                        quotes
                            .iter()
                            .map(|(q, _)| {
                                let mut f = Facts::new();
                                f.insert(
                                    "number".into(),
                                    Value::String(
                                        truthy(q.full_number.as_deref())
                                            .unwrap_or(&q.number)
                                            .to_string(),
                                    ),
                                );
                                f.insert("total".into(), Value::String(str_decimal_opt(q.total)));
                                f.insert("currency".into(), s_(q.currency.as_deref()));
                                Value::Object(f)
                            })
                            .collect(),
                    ),
                );
                put("content_sha256", Value::String(content));
            }
            Entity::LibraryDocumentVersion(v) => {
                put("title", Value::String(v.document.title.clone()));
                put("label", Value::String(v.row.label.clone()));
                put("change_note", Value::String(v.row.change_note.clone()));
                put("file_name", Value::String(v.row.file_name.clone()));
                put("file_sha256", Value::String(v.row.file_sha256.clone()));
                put(
                    "audiences",
                    Value::Array(
                        v.document
                            .audiences
                            .iter()
                            .cloned()
                            .map(Value::String)
                            .collect(),
                    ),
                );
            }
        }
        Ok(m)
    }

    /// `extra_snapshot(db, entity, facts)`.
    pub async fn extra_snapshot(
        &self,
        db: &mut PgConnection,
        e: &Entity,
        facts: &Facts,
    ) -> Result<Facts, ApiError> {
        match e {
            Entity::Quotation(qd) => quotation_extra(db, qd).await,
            Entity::SalesContract(c) => {
                money(db, f0(c.value), c.currency.as_deref(), c.start_date, None).await
            }
            Entity::Invoice(inv) => invoice_extra(db, inv, facts).await,
            Entity::Expense(x) => {
                money(
                    db,
                    dec_to_f64(x.amount),
                    x.currency.as_deref(),
                    Some(x.expense_date),
                    None,
                )
                .await
            }
            Entity::PurchaseOrder(pd) => purchase_order_extra(db, pd, facts).await,
            Entity::Bom(bd) => {
                match bom_cost(db, &bd.items, Decimal::ONE).await {
                    Ok((per_unit, currency)) => {
                        money(db, dec_to_f64(per_unit), Some(&currency), None, None).await
                    }
                    // a component's currency has no FX rate: the Owner step applies
                    Err(ApiError::Http { .. }) => {
                        let mut m = Facts::new();
                        m.insert("amount".into(), fl(0.0));
                        m.insert("currency".into(), Value::String("USD".into()));
                        m.insert("amount_base".into(), Value::Null);
                        m.insert("fx_rate_base".into(), Value::Null);
                        m.insert("fx_rate_missing".into(), Value::Bool(true));
                        m.insert("max_line_amount_base".into(), Value::Null);
                        Ok(m)
                    }
                    Err(other) => Err(other),
                }
            }
            Entity::ContractorRequest(r) => {
                money(db, f0(r.estimated_value), r.currency.as_deref(), None, None).await
            }
            Entity::WorkOrder(wo) => work_order_extra(db, wo).await,
            Entity::VendorInvoice(vi) => vendor_invoice_extra(db, vi).await,
            Entity::MarketingContent(c) => {
                let mut m = Facts::new();
                m.insert("content_type".into(), s_(c.content_type.as_deref()));
                Ok(m)
            }
            Entity::OfferLetter(o) => {
                money(
                    db,
                    dec_to_f64(o.salary_gross_monthly),
                    o.salary_currency.as_deref(),
                    o.start_date,
                    None,
                )
                .await
            }
            Entity::PayrollRun(p) => {
                // date(p.year, p.month, 1): a bad month/year is a ValueError in Python (500).
                let first = NaiveDate::from_ymd_opt(p.year, p.month as u32, 1)
                    .ok_or_else(|| internal("ValueError: month must be in 1..12"))?;
                money(
                    db,
                    f0(p.total_gross),
                    p.currency.as_deref(),
                    Some(first),
                    None,
                )
                .await
            }
            Entity::HiringRequest(_)
            | Entity::SopArticle(_)
            | Entity::SalesDocument(_)
            | Entity::LibraryDocumentVersion(_) => Ok(Facts::new()),
        }
    }

    /// `snapshot(db, entity)`: `{"document": facts, **extra}`.
    pub async fn snapshot(&self, db: &mut PgConnection, e: &Entity) -> Result<Facts, ApiError> {
        let facts = self.document_facts(db, e).await?;
        let extra = self.extra_snapshot(db, e, &facts).await?;
        let mut snap = Facts::new();
        snap.insert("document".into(), Value::Object(facts));
        for (k, v) in extra {
            snap.insert(k, v);
        }
        Ok(snap)
    }

    // ─── hooks ─────────────────────────────────────────────────────────────

    /// `on_submitted(db, entity, user)`.
    pub async fn on_submitted(
        &self,
        db: &mut PgConnection,
        e: &mut Entity,
        _user: &AuthUser,
    ) -> Result<(), ApiError> {
        let mut d = Dirty::default();
        let id = e.id();
        let table = match e {
            Entity::Expense(x) => {
                set!(d, x, status, Some("pending".to_string()));
                Expense::TABLE
            }
            Entity::PurchaseOrder(pd) => {
                set!(d, pd.row, approval_status, Some("pending".to_string()));
                PurchaseOrder::TABLE
            }
            Entity::ContractorRequest(r) => {
                set!(d, r, status, Some("pending_approval".to_string()));
                ContractorRequest::TABLE
            }
            Entity::VendorInvoice(vi) => {
                set!(d, vi, status, Some("under_review".to_string()));
                VendorInvoice::TABLE
            }
            Entity::MarketingContent(c) => {
                set!(d, c, status, Some("review".to_string()));
                ContentCalendarItem::TABLE
            }
            Entity::HiringRequest(h) => {
                set!(d, h, status, Some("pending_approval".to_string()));
                HiringRequest::TABLE
            }
            Entity::SopArticle(a) => {
                set!(d, a, status, Some("review".to_string()));
                KBArticle::TABLE
            }
            Entity::SalesDocument(doc) => {
                set!(d, doc, status, "review".to_string());
                SalesDocument::TABLE
            }
            // doclib A1 (the versions table has no updated_at: written here, not through flush)
            Entity::LibraryDocumentVersion(v) => {
                sqlx::query(
                    "UPDATE library_document_versions SET status = 'pending_approval' WHERE id = $1",
                )
                .bind(v.row.id)
                .execute(&mut *db)
                .await?;
                v.row.status = "pending_approval".into();
                return Ok(());
            }
            _ => return Ok(()),
        };
        flush(db, table, id, d).await
    }

    /// `on_cancelled(db, entity)`: the pending request was withdrawn or auto-cancelled: undo what
    /// on_submitted set.
    pub async fn on_cancelled(
        &self,
        db: &mut PgConnection,
        e: &mut Entity,
    ) -> Result<(), ApiError> {
        let mut d = Dirty::default();
        let id = e.id();
        let table = match e {
            Entity::PurchaseOrder(pd) => {
                if pd.row.approval_status.as_deref() == Some("pending") {
                    // not receivable until resubmitted and approved
                    set!(d, pd.row, approval_status, Some("cancelled".to_string()));
                }
                PurchaseOrder::TABLE
            }
            Entity::VendorInvoice(vi) => {
                if vi.status.as_deref() == Some("under_review") {
                    set!(d, vi, status, Some("submitted".to_string()));
                }
                VendorInvoice::TABLE
            }
            Entity::MarketingContent(c) => {
                if c.status.as_deref() == Some("review") {
                    set!(d, c, status, Some("in_progress".to_string()));
                }
                ContentCalendarItem::TABLE
            }
            Entity::SopArticle(a) => {
                if a.status.as_deref() == Some("review") {
                    set!(d, a, status, Some("draft".to_string()));
                }
                KBArticle::TABLE
            }
            Entity::SalesDocument(doc) => {
                if doc.status == "review" {
                    set!(d, doc, status, "draft".to_string());
                }
                SalesDocument::TABLE
            }
            // doclib A1: a withdrawn version is not coming back
            Entity::LibraryDocumentVersion(v) => {
                crate::doc_library::versions::mark_rejected(
                    db,
                    v.row.id,
                    Some("Withdrawn before approval"),
                )
                .await?;
                return Ok(());
            }
            _ => return Ok(()),
        };
        flush(db, table, id, d).await
    }

    /// `on_decision(db, entity, decision, actor, comment)`.
    pub async fn on_decision(
        &self,
        db: &mut PgConnection,
        e: &mut Entity,
        decision: &str,
        actor: Option<&AuthUser>,
        comment: Option<&str>,
    ) -> Result<(), ApiError> {
        let actor_id = actor.map(|a| a.id);
        let rejection = if decision == "rejected" {
            comment.map(str::to_string)
        } else {
            None
        };
        let mut d = Dirty::default();
        match e {
            Entity::Quotation(qd) => {
                if decision == "approved" {
                    set!(d, qd.row, approved_by_id, actor_id);
                }
                flush(db, Quotation::TABLE, qd.row.id, d).await
            }
            Entity::Invoice(inv) => {
                if let (true, Some(a)) = (decision == "approved", actor) {
                    set!(d, inv, authorized_by_id, Some(a.id));
                    set!(d, inv, authorized_by_name, Some(a.name.clone()));
                }
                flush(db, Invoice::TABLE, inv.id, d).await
            }
            Entity::Expense(x) => {
                set!(d, x, status, Some(decision.to_string()));
                set!(d, x, approved_by_id, actor_id);
                set!(d, x, approved_at, Some(now_utc()));
                set!(d, x, rejection_reason, rejection);
                flush(db, Expense::TABLE, x.id, d).await?;
                if decision == "approved" {
                    // Dr expense / Cr employee reimbursements payable
                    crate::gl_posting::post_expense_approved(db, x, actor_id).await?;
                }
                Ok(())
            }
            Entity::PurchaseOrder(pd) => {
                set!(d, pd.row, approval_status, Some(decision.to_string()));
                if decision == "approved" {
                    set!(d, pd.row, approved_by_id, actor_id);
                    set!(d, pd.row, approved_at, Some(now_utc()));
                }
                flush(db, PurchaseOrder::TABLE, pd.row.id, d).await
            }
            Entity::Bom(bd) => {
                if decision != "approved" {
                    set!(d, bd.row, status, Some("draft".to_string()));
                    return flush(db, BOM::TABLE, bd.row.id, d).await;
                }
                // The product's previously approved BOM is archived: one approved BOM per product.
                let others: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM boms WHERE product_id = $1 AND status = 'approved' AND id != $2",
                )
                .bind(bd.row.product_id)
                .bind(bd.row.id)
                .fetch_all(&mut *db)
                .await?;
                set!(d, bd.row, status, Some("approved".to_string()));
                set!(d, bd.row, approved_by_id, actor_id);
                // The unit of work writes a mapper's UPDATEs in primary key order; keep that order so the
                // rows land in the same heap order.
                let mut order: Vec<Uuid> = others.clone();
                order.push(bd.row.id);
                order.sort();
                let mut mine = Some(d);
                for id in order {
                    if id == bd.row.id {
                        flush(db, BOM::TABLE, id, mine.take().unwrap_or_default()).await?;
                    } else {
                        sqlx::query(
                            "UPDATE boms SET status = 'archived', updated_at = now() WHERE id = $1",
                        )
                        .bind(id)
                        .execute(&mut *db)
                        .await?;
                    }
                }
                Ok(())
            }
            Entity::ContractorRequest(r) => {
                set!(d, r, status, Some(decision.to_string()));
                set!(d, r, approved_by_id, actor_id);
                set!(d, r, approved_at, Some(now_utc()));
                set!(d, r, rejection_reason, rejection);
                flush(db, ContractorRequest::TABLE, r.id, d).await
            }
            Entity::VendorInvoice(vi) => {
                set!(d, vi, status, Some(decision.to_string()));
                set!(d, vi, reviewed_by_id, actor_id);
                set!(d, vi, reviewed_at, Some(now_utc()));
                set!(d, vi, rejection_reason, rejection);
                flush(db, VendorInvoice::TABLE, vi.id, d).await?;
                if decision == "approved" {
                    // Dr cost + sales tax input / Cr AP + withholding tax payable
                    // 3-way match: what has been invoiced per PO line
                    apply_invoiced_quantities(db, vi).await?;
                    let mut doc = crate::gl_posting::VendorInvoiceDoc::from(&**vi);
                    let posted =
                        crate::gl_posting::post_vendor_invoice_approved(db, &mut doc, actor_id)
                            .await;
                    // post_vendor_invoice_approved writes the bill's rate to the row itself; keep the entity in step.
                    vi.exchange_rate = doc.exchange_rate;
                    posted?;
                }
                Ok(())
            }
            Entity::MarketingContent(c) => {
                let status = if decision == "approved" {
                    "approved"
                } else {
                    "in_progress"
                };
                set!(d, c, status, Some(status.to_string()));
                flush(db, ContentCalendarItem::TABLE, c.id, d).await
            }
            Entity::HiringRequest(h) => {
                if decision == "approved" {
                    set!(d, h, status, Some("approved".to_string()));
                    set!(d, h, approved_by_id, actor_id);
                    set!(d, h, approved_at, Some(now_utc()));
                } else {
                    set!(d, h, status, Some("on_hold".to_string()));
                }
                flush(db, HiringRequest::TABLE, h.id, d).await
            }
            Entity::SopArticle(a) => {
                if decision == "rejected" {
                    set!(d, a, status, Some("draft".to_string()));
                } else if a.author_signed_at.is_some() && a.owner_signed_at.is_some() {
                    publish_sop_article(db, a, actor).await?;
                    return Ok(());
                } else {
                    // approved, but it publishes only once both signatures are on it
                    set!(d, a, status, Some("draft".to_string()));
                }
                flush(db, KBArticle::TABLE, a.id, d).await
            }
            Entity::PayrollRun(p) => {
                if decision == "approved" {
                    set!(d, p, status, Some("approved".to_string()));
                    set!(d, p, approved_by_id, actor_id);
                    set!(d, p, approved_at, Some(now_utc()));
                }
                flush(db, PayrollRun::TABLE, p.id, d).await
            }
            Entity::SalesDocument(doc) => {
                if decision == "approved" {
                    set!(d, doc, status, "approved".to_string());
                    let issue = match doc.issue_date {
                        Some(x) => x,
                        None => today_local(db).await?,
                    };
                    set!(d, doc, issue_date, Some(issue));
                } else {
                    set!(d, doc, status, "draft".to_string());
                }
                flush(db, SalesDocument::TABLE, doc.id, d).await
            }
            // doclib A1: approved -> live (the live version before is superseded); rejected -> rejected
            Entity::LibraryDocumentVersion(v) => {
                if decision == "approved" {
                    crate::doc_library::versions::go_live(db, v.row.id, actor_id).await
                } else {
                    crate::doc_library::versions::mark_rejected(db, v.row.id, comment).await
                }
            }
            Entity::SalesContract(_) | Entity::WorkOrder(_) | Entity::OfferLetter(_) => Ok(()),
        }
    }
}

/// `str(Decimal)` of a NUMERIC read through rust_decimal (`None` prints as "None").
fn str_decimal_opt(v: Option<Decimal>) -> String {
    match v {
        None => "None".to_string(),
        Some(d) => pyjson::PyDecimal::parse(&d.to_string())
            .map(|p| pyjson::str_decimal(&p))
            .unwrap_or_else(|_| d.to_string()),
    }
}

// ─── item facts ───────────────────────────────────────────────────────────────

fn nonzero(v: Option<Decimal>) -> bool {
    v.map(|d| dec_to_f64(d) != 0.0).unwrap_or(false)
}

/// `_invoice_item_facts(i)`.
fn invoice_item_facts(i: &InvoiceItem) -> Facts {
    let mut f = Facts::new();
    f.insert("name".into(), Value::String(i.name.clone()));
    f.insert("quantity".into(), fd(i.quantity));
    f.insert("unit_price".into(), fd(i.unit_price));
    f.insert("amount".into(), fd(i.amount));
    // Line discount/tax join the fingerprint only when set, so invoices approved before these
    // fields existed (all zero) keep their fingerprint and their approval.
    for (key, val) in [
        ("discount_pct", i.discount_pct),
        ("tax_pct", i.tax_pct),
        ("tax_amount", i.tax_amount),
    ] {
        if nonzero(Some(val)) {
            f.insert(key.into(), fd(val));
        }
    }
    f
}

/// `_quotation_item_facts(i)`.
fn quotation_item_facts(i: &QuotationItem) -> Facts {
    let mut f = Facts::new();
    f.insert("product_id".into(), id_(i.product_id));
    f.insert("sku".into(), s_(i.sku.as_deref()));
    f.insert("name".into(), Value::String(i.name.clone()));
    f.insert("quantity".into(), fd(i.quantity));
    f.insert("unit".into(), s_(i.unit.as_deref()));
    f.insert("unit_price".into(), fd(i.unit_price));
    f.insert("discount_pct".into(), f_(i.discount_percentage));
    f.insert(
        "discount_type".into(),
        Value::String(
            truthy(i.discount_type.as_deref())
                .unwrap_or("none")
                .to_string(),
        ),
    );
    f.insert(
        "is_optional".into(),
        Value::Bool(i.is_optional.unwrap_or(false)),
    );
    // Term, add-on parent and line tax join the fingerprint only when set, so quotations approved
    // before these fields existed keep their fingerprint and their approval.
    if let Some(t) = i.term_months.filter(|t| *t != 0) {
        f.insert("term_months".into(), Value::from(t));
    }
    if let Some(p) = i.parent_item_id {
        f.insert("parent_item_id".into(), Value::String(p.to_string()));
    }
    if nonzero(Some(i.tax_pct)) {
        f.insert("tax_pct".into(), fd(i.tax_pct));
    }
    f
}

// ─── money ────────────────────────────────────────────────────────────────────

/// `to_base(db, amount, currency, on_date)`: convert to the functional currency. Returns (amount_base, rate), (None, None)
/// when no rate exists. `amount` is `float(amount or 0)`.
pub async fn to_base(
    db: &mut PgConnection,
    amount: f64,
    currency: Option<&str>,
    on_date: Option<NaiveDate>,
) -> Result<(Option<f64>, Option<f64>), ApiError> {
    let value = amount;
    let functional = crate::fx::functional_currency(db).await?;
    let cur = truthy(currency)
        .map(str::to_string)
        .unwrap_or_else(|| functional.clone())
        .to_uppercase();
    if cur == functional {
        return Ok((Some(value), Some(1.0)));
    }
    let on = match on_date {
        Some(d) => d,
        None => today_local(db).await?,
    };
    match crate::fx::get_rate(db, &cur, &functional, Some(on)).await {
        Ok(rate) => {
            let rate = dec_to_f64(rate);
            Ok((Some(value * rate), Some(rate)))
        }
        Err(crate::fx::RateError::NoRate(_)) => Ok((None, None)),
        Err(crate::fx::RateError::Other(e)) => Err(e),
    }
}

/// `money(db, amount, currency, on_date, line_amounts)`: amount facts in the functional currency. `line_amounts` (document
/// currency) gives the largest single line for `single_line_gt_base`; a document without lines counts as
/// one line of its total.
pub async fn money(
    db: &mut PgConnection,
    amount: f64,
    currency: Option<&str>,
    on_date: Option<NaiveDate>,
    line_amounts: Option<&[Option<f64>]>,
) -> Result<Facts, ApiError> {
    let (amount_base, rate) = to_base(db, amount, currency, on_date).await?;
    let mut max_line_base = amount_base;
    if let Some(lines) = line_amounts {
        let largest = py_max(lines.iter().map(|v| v.unwrap_or(0.0)));
        max_line_base = rate.map(|r| largest * r);
    }
    let cur = truthy(currency).unwrap_or("USD").to_uppercase();
    let mut m = Facts::new();
    m.insert("amount".into(), fl(amount));
    m.insert("currency".into(), Value::String(cur));
    m.insert("amount_base".into(), flo(amount_base));
    m.insert("fx_rate_base".into(), flo(rate));
    m.insert("fx_rate_missing".into(), Value::Bool(amount_base.is_none()));
    m.insert("max_line_amount_base".into(), flo(max_line_base));
    Ok(m)
}

// ─── payment terms ────────────────────────────────────────────────────────────

/// `_NET_DAYS = re.compile(r"net\s*[-:]?\s*(\d{1,3})|(\d{1,3})\s*days?", re.IGNORECASE)`, with
/// Python's `\s` set.
static NET_DAYS: Lazy<Regex> = Lazy::new(|| {
    let s = format!("[{}]", pystr::PY_SPACE_CLASS);
    Regex::new(&format!(
        r"(?i)net{s}*[-:]?{s}*(\d{{1,3}})|(\d{{1,3}}){s}*days?"
    ))
    .expect("NET_DAYS")
});

/// Python `int()` of a matched run of decimal digits (any script).
fn digits_value(s: &str) -> Option<i64> {
    let mut v: i64 = 0;
    for c in s.chars() {
        v = v * 10 + pystr::decimal_value(c)? as i64;
    }
    Some(v)
}

/// Python `int(str)`: whitespace, sign, decimal digits of any script, single underscores between digits.
fn py_int_str(s: &str) -> Option<i64> {
    let t = pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut v: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        v = v
            .checked_mul(10)?
            .checked_add(pystr::decimal_value(c)? as i64)?;
    }
    Some(if neg { -v } else { v })
}

/// Python truthiness of a JSON value.
fn json_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(v)` of a value json.loads produced.
fn py_str(v: &Value) -> String {
    pyjson::str_value(&PyValue::from(v))
}

/// `" ".join(str(v) for v in (x.values() if isinstance(x, dict) else [x]))`.
fn joined_text(x: &Value) -> String {
    match x {
        Value::Object(o) => o.values().map(py_str).collect::<Vec<_>>().join(" "),
        other => py_str(other),
    }
}

/// `derive_terms_days(payment_terms)`: longest net-days figure in a quotation's payment_terms JSON
/// ({"net_days": 30} or {"text": "NET30"}).
pub fn derive_terms_days(payment_terms: Option<&Value>) -> Option<i64> {
    let pt = payment_terms?;
    if !json_truthy(pt) {
        return None;
    }
    if let Value::Object(o) = pt {
        if let Some(nd) = o.get("net_days") {
            if !matches!(nd, Value::Null) && nd != &Value::String(String::new()) {
                // int(...): TypeError / ValueError fall through to the text search.
                let parsed = match PyValue::from(nd) {
                    PyValue::Int(i) => i64::try_from(i).ok(),
                    PyValue::Float(f) => Some(f.trunc() as i64),
                    PyValue::Bool(b) => Some(b as i64),
                    PyValue::Str(s) => py_int_str(&s),
                    _ => None,
                };
                if let Some(v) = parsed {
                    return Some(v);
                }
            }
        }
    }
    let text = joined_text(pt);
    NET_DAYS
        .captures_iter(&text)
        .filter_map(|c| {
            let m = c
                .get(1)
                .filter(|m| !m.as_str().is_empty())
                .or_else(|| c.get(2))?;
            digits_value(m.as_str())
        })
        .max()
}

// ─── extra snapshots ──────────────────────────────────────────────────────────

/// 3 Nov 2026 (Owner): the lines of a revision that keep exactly the price of the same line in the quotation it
/// replaces (`previous_version_id`, the sent one). Such a line is not a new discount: the
/// `missing_discount_type` precondition lets it through (`kept_price`), though the catalogue price may have
/// risen since; the approval itself runs as usual. "The same line": the same product (or, for a typed line,
/// name), unit and term; "the same price": unit price, line discount and the header discount all unchanged.
/// A new line, or one whose price changed, is checked as before.
async fn kept_price_lines(
    db: &mut PgConnection,
    q: &Quotation,
    items: &[QuotationItem],
) -> Result<std::collections::HashSet<Uuid>, ApiError> {
    let mut kept = std::collections::HashSet::new();
    let Some(prev_id) = q.previous_version_id else {
        return Ok(kept);
    };
    let prev: Option<(Option<String>, Option<Decimal>)> = sqlx::query_as(
        "SELECT discount_type, discount_value FROM quotations WHERE id = $1 AND is_deleted = false",
    )
    .bind(prev_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((prev_dtype, prev_dvalue)) = prev else {
        return Ok(kept);
    };
    let header = |t: &Option<String>, v: Option<Decimal>| -> (String, Decimal) {
        let v = v.unwrap_or_default();
        if v.is_zero() {
            (String::new(), Decimal::ZERO)
        } else {
            (truthy(t.as_deref()).unwrap_or("").to_string(), v)
        }
    };
    if header(&q.discount_type, q.discount_value) != header(&prev_dtype, prev_dvalue) {
        return Ok(kept);
    }
    let before: Vec<QuotationItem> =
        sqlx::query_as("SELECT * FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order")
            .bind(prev_id)
            .fetch_all(&mut *db)
            .await?;
    let unit = |i: &QuotationItem| truthy(i.unit.as_deref()).unwrap_or("").to_lowercase();
    for i in items {
        let same = before.iter().any(|b| {
            let same_line = match (i.product_id, b.product_id) {
                (Some(a), Some(c)) => a == c,
                (None, None) => i.name.trim() == b.name.trim(),
                _ => false,
            };
            same_line
                && unit(i) == unit(b)
                && i.term_months.unwrap_or(0) == b.term_months.unwrap_or(0)
                && i.unit_price == b.unit_price
                && i.discount_percentage.unwrap_or_default()
                    == b.discount_percentage.unwrap_or_default()
        });
        if same {
            kept.insert(i.id);
        }
    }
    Ok(kept)
}

async fn quotation_extra(db: &mut PgConnection, qd: &QuotationDoc) -> Result<Facts, ApiError> {
    let q = &qd.row;
    // Thresholds are on the value before tax (quotations carry per-line tax since migration 081)
    let net = D(q.total) - D(q.tax_amount);
    let mut snap = money(
        db,
        dec_to_f64(net),
        q.currency.as_deref(),
        Some(q.issue_date),
        None,
    )
    .await?;

    #[derive(sqlx::FromRow)]
    struct AllowanceRow {
        kind: String,
        label: String,
        min_qty: Option<Decimal>,
        max_qty: Option<Decimal>,
        max_discount_pct: Decimal,
        notify_owner: bool,
    }
    let rows: Vec<AllowanceRow> = sqlx::query_as(
        "SELECT kind, label, min_qty, max_qty, max_discount_pct, notify_owner FROM discount_allowances \
         WHERE is_active = true",
    )
    .fetch_all(&mut *db)
    .await?;
    struct Allowance {
        kind: String,
        min_qty: Option<f64>,
        max_qty: Option<f64>,
        max_discount_pct: f64,
        json: Value,
    }
    let allowances: Vec<Allowance> = rows
        .into_iter()
        .map(|a| {
            let mut m = Facts::new();
            m.insert("kind".into(), Value::String(a.kind.clone()));
            m.insert("label".into(), Value::String(a.label.clone()));
            m.insert("min_qty".into(), f_(a.min_qty));
            m.insert("max_qty".into(), f_(a.max_qty));
            m.insert("max_discount_pct".into(), fd(a.max_discount_pct));
            m.insert("notify_owner".into(), Value::Bool(a.notify_owner));
            Allowance {
                kind: a.kind,
                min_qty: fv(a.min_qty),
                max_qty: fv(a.max_qty),
                max_discount_pct: dec_to_f64(a.max_discount_pct),
                json: Value::Object(m),
            }
        })
        .collect();

    let mut is_loyal = false;
    if let Some(org) = q.customer_org_id {
        let won: Vec<String> = crate::crm::stages::WON_STAGES
            .iter()
            .map(|s| s.to_string())
            .collect();
        is_loyal = sqlx::query(
            "SELECT id FROM deals WHERE organization_id = $1 AND stage = ANY($2) LIMIT 1",
        )
        .bind(org)
        .bind(&won)
        .fetch_optional(&mut *db)
        .await?
        .is_some();
    }

    let mut header_pct = 0.0;
    let subtotal = f0(q.subtotal);
    match q.discount_type.as_deref() {
        Some("percentage") => header_pct = f0(q.discount_value),
        Some("fixed") if subtotal > 0.0 => header_pct = f0(q.discount_value) / subtotal * 100.0,
        _ => {}
    }

    // The expected price of every line, from the pricing service (the price valid on the issue
    // date, the SKU's volume tier, add-on percentages, white-label). A line is at list when it is a
    // catalog price with at most its tier discount; anything else is a discount or a manual price.
    let items = &qd.items;
    let mut priced: Vec<crate::pricing::PricedLine> = Vec::new();
    if !items.is_empty() {
        let lines: Vec<crate::pricing::LineIn> = items
            .iter()
            .map(|i| {
                let mut l = crate::pricing::LineIn::new(i.id.to_string());
                l.product_id = i.product_id.map(|p| p.to_string());
                l.name = Some(truthy(Some(i.name.as_str())).unwrap_or("Line").to_string());
                l.unit = i.unit.clone();
                l.quantity = i.quantity;
                l.unit_price = Some(i.unit_price);
                l.discount_pct = Some(i.discount_percentage.unwrap_or(Decimal::ZERO));
                l.discount_type = i.discount_type.clone();
                l.is_optional = i.is_optional.unwrap_or(false);
                l.parent_key = i.parent_item_id.map(|p| p.to_string());
                l.term_months = i.term_months.map(i64::from);
                l.tax_pct = Some(Decimal::ZERO);
                l
            })
            .collect();
        let org = q.customer_org_id.map(|o| o.to_string());
        let res = crate::pricing::price_lines(
            db,
            truthy(q.currency.as_deref()).unwrap_or("USD"),
            q.issue_date,
            org.as_deref(),
            &lines,
            None,
            false,
            false,
            None,
            None,
            None,
        )
        .await?;
        priced = res.lines;
    }

    let kept = kept_price_lines(db, q, items).await?;
    let functional = crate::fx::functional_currency(db).await?;
    let mut lines: Vec<Facts> = Vec::new();
    let (mut revenue, mut cost) = (0.0_f64, 0.0_f64);
    let (mut margin_unknown, mut has_custom, mut has_physical, mut has_multi_year_service) =
        (false, false, false, false);
    // Standing authority (Owner rule): every line (optional ones too, the customer may take them) is a
    // catalog product at its list price (a volume tier is list) with no other line or header discount.
    let mut catalog_list_price = header_pct <= 0.0;
    for item in items {
        let key = item.id.to_string();
        let p = priced
            .iter()
            .find(|p| p.key == key)
            .ok_or_else(|| internal(format!("KeyError: '{key}'")))?;
        let qty = dec_to_f64(item.quantity);
        let unit_price = dec_to_f64(item.unit_price);
        let line_pct = f0(item.discount_percentage);
        let periods = dec_to_f64(p.periods);
        let net_unit = unit_price * (1.0 - line_pct / 100.0) * (1.0 - header_pct / 100.0);

        // Expected net unit price: list less the SKU's volume tier (None for a manual / custom line)
        let expected = p
            .list_unit_price
            .map(|l| dec_to_f64(l) * (1.0 - dec_to_f64(p.tier_pct) / 100.0));
        if !p.is_list {
            catalog_list_price = false;
        }
        // Discount beyond list less tier: catches a price typed below list with "0%" discount
        let effective = if p.is_list && header_pct <= 0.0 {
            0.0
        } else if let Some(exp) = expected.filter(|x| *x != 0.0) {
            f64::max(0.0, (1.0 - net_unit / exp) * 100.0)
        } else {
            f64::max(
                0.0,
                (1.0 - (1.0 - line_pct / 100.0) * (1.0 - header_pct / 100.0)) * 100.0,
            )
        };
        let effective = py_round2(effective);

        let kind = truthy(item.discount_type.as_deref())
            .unwrap_or("none")
            .to_string();
        let mut allowance = Value::Null;
        if effective > 0.0 && kind != "none" && !(kind == "loyalty" && !is_loyal) {
            let mut best: Option<&Allowance> = None;
            for a in allowances.iter().filter(|a| {
                a.kind == kind
                    && a.min_qty.map(|m| qty >= m).unwrap_or(true)
                    && a.max_qty.map(|m| qty <= m).unwrap_or(true)
            }) {
                // max(matches, key=...): the first of equal maxima wins
                if best
                    .map(|b| a.max_discount_pct > b.max_discount_pct)
                    .unwrap_or(true)
                {
                    best = Some(a);
                }
            }
            if let Some(b) = best {
                allowance = b.json.clone();
            }
        }

        // Custom: not a catalog item, or a catalog item priced by hand (manual / quote-only)
        if item.product_id.is_none() || (p.price_rule == "manual" && p.list_unit_price.is_none()) {
            has_custom = true;
        }
        if item.product_id.is_some() && !item.is_optional.unwrap_or(false) {
            if p.product_type.as_deref() == Some("physical") {
                has_physical = true;
            }
            if matches!(p.product_type.as_deref(), Some("saas" | "service"))
                && p.sale_model == "recurring"
                && p.term_months.unwrap_or(0) >= 24
            {
                has_multi_year_service = true;
            }
            let line_revenue = qty * net_unit * periods;
            match p.cost_price {
                None => margin_unknown = true,
                Some(cp) => {
                    // cost_price is per unit (per rate period for a recurring item), in the functional currency
                    let mut cost_value = Some(dec_to_f64(cp) * qty * periods);
                    if q.currency.as_deref().unwrap_or("").to_uppercase() != functional {
                        let (converted, _) = to_base(
                            db,
                            cost_value.unwrap_or(0.0),
                            Some(&functional),
                            Some(q.issue_date),
                        )
                        .await?;
                        let (to_quote, _) =
                            to_base(db, 1.0, q.currency.as_deref(), Some(q.issue_date)).await?;
                        match (converted, to_quote) {
                            (Some(c), Some(t)) if t != 0.0 => cost_value = Some(c / t),
                            _ => {
                                margin_unknown = true;
                                cost_value = None;
                            }
                        }
                    }
                    if let Some(cv) = cost_value {
                        revenue += line_revenue;
                        cost += cv;
                    }
                }
            }
        }

        let mut line = Facts::new();
        line.insert("name".into(), Value::String(item.name.clone()));
        line.insert("quantity".into(), fl(qty));
        line.insert("unit_price".into(), fl(unit_price));
        line.insert(
            "list_price".into(),
            p.list_unit_price.map(fd).unwrap_or(Value::Null),
        );
        line.insert("discount_pct".into(), fl(effective));
        line.insert("discount_type".into(), Value::String(kind));
        line.insert("is_custom".into(), Value::Bool(item.product_id.is_none()));
        line.insert("price_rule".into(), Value::String(p.price_rule.clone()));
        line.insert("is_list".into(), Value::Bool(p.is_list));
        line.insert("allowance".into(), allowance);
        line.insert("amount".into(), fd(item.amount));
        // only when true, so the snapshots of quotations that are not revisions stay as they were
        if kept.contains(&item.id) {
            line.insert("kept_price".into(), Value::Bool(true));
        }
        lines.push(line);
    }

    let line_f = |l: &Facts, k: &str| -> f64 {
        l.get(k)
            .and_then(|v| match v {
                Value::Number(n) => n.as_f64(),
                _ => None,
            })
            .unwrap_or(0.0)
    };
    // Largest single line, net of line and header discount, in the functional currency (single_line_gt_base)
    let rate = match snap.get("fx_rate_base") {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    };
    snap.insert(
        "max_line_amount_base".into(),
        match rate {
            None => Value::Null,
            Some(r) => {
                fl(py_max(lines.iter().map(|l| {
                    line_f(l, "amount") * (1.0 - header_pct / 100.0) * r
                })))
            }
        },
    );
    let max_discount = py_max(lines.iter().map(|l| line_f(l, "discount_pct")));
    let n_lines = lines.len();
    snap.insert(
        "lines".into(),
        Value::Array(lines.into_iter().map(Value::Object).collect()),
    );
    snap.insert(
        "allowances".into(),
        Value::Array(allowances.iter().map(|a| a.json.clone()).collect()),
    );
    snap.insert("is_loyal_customer".into(), Value::Bool(is_loyal));
    snap.insert("max_discount_pct".into(), fl(max_discount));
    snap.insert(
        "margin_pct".into(),
        if revenue > 0.0 {
            fl(py_round2((revenue - cost) / revenue * 100.0))
        } else {
            Value::Null
        },
    );
    snap.insert("margin_unknown".into(), Value::Bool(margin_unknown));
    snap.insert("has_custom_line".into(), Value::Bool(has_custom));
    // 5 Nov 2026 (Owner decision): a line whose catalog price is still proposed, or a SKU that always goes to the Owner
    // (sales.pricing_rules.owner_approval_skus: the Dedicated Engineering retainers), is never standing authority
    if catalog_list_price && crate::pricing_rules::needs_owner(db, q.id).await? {
        catalog_list_price = false;
    }
    snap.insert(
        "all_catalog_list_price".into(),
        Value::Bool(catalog_list_price && n_lines > 0),
    );
    snap.insert(
        "is_multi_year_bundle".into(),
        Value::Bool(has_physical && has_multi_year_service),
    );
    snap.insert(
        "terms_days".into(),
        match q.payment_terms_days {
            Some(d) => Value::from(d),
            None => derive_terms_days(q.payment_terms.as_ref())
                .map(Value::from)
                .unwrap_or(Value::Null),
        },
    );
    snap.insert(
        "procurement_type".into(),
        Value::String(
            truthy(q.procurement_type.as_deref())
                .unwrap_or("standard")
                .to_string(),
        ),
    );
    Ok(snap)
}

/// `re.sub(r"\s+", " ", t).strip().lower()`.
fn norm_terms(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut in_space = false;
    for c in t.chars() {
        if pystr::is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    pystr::lower(pystr::strip(&out))
}

async fn invoice_extra(
    db: &mut PgConnection,
    inv: &Invoice,
    facts: &Facts,
) -> Result<Facts, ApiError> {
    let line_amounts: Vec<Option<f64>> = match facts.get("items") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| match i.get("amount") {
                Some(Value::Number(n)) => n.as_f64(),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut snap = money(
        db,
        f0(inv.total),
        inv.currency.as_deref(),
        Some(inv.issue_date),
        Some(&line_amounts),
    )
    .await?;

    let mut terms_differ = false;
    if let (Some(qid), Some(text)) = (inv.quotation_id, truthy(inv.payment_terms_text.as_deref())) {
        let quoted: Option<(Option<Value>,)> =
            sqlx::query_as("SELECT payment_terms FROM quotations WHERE id = $1")
                .bind(qid)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(quoted) = quoted.and_then(|r| r.0).filter(json_truthy) {
            let quoted_text = joined_text(&quoted);
            terms_differ = norm_terms(&quoted_text) != norm_terms(text);
        }
    }

    let org = inv.customer_org_id;
    let name = truthy(inv.customer_name.as_deref());
    let base = "AND id != $2 AND is_deleted = false AND status NOT IN ('draft', 'cancelled', 'superseded')";
    let prior: i64 = if let Some(org) = org {
        sqlx::query_scalar(&format!(
            "SELECT count(*) FROM invoices WHERE customer_org_id = $1 {base}"
        ))
        .bind(org)
        .bind(inv.id)
        .fetch_one(&mut *db)
        .await?
    } else if let Some(name) = name {
        sqlx::query_scalar(&format!(
            "SELECT count(*) FROM invoices WHERE customer_name = $1 {base}"
        ))
        .bind(name)
        .bind(inv.id)
        .fetch_one(&mut *db)
        .await?
    } else {
        1
    };

    snap.insert(
        "is_credit_note".into(),
        Value::Bool(inv.invoice_type.as_deref() == Some("credit_note")),
    );
    snap.insert(
        "terms_differ_from_contract".into(),
        Value::Bool(terms_differ),
    );
    snap.insert(
        "is_first_invoice_for_customer".into(),
        Value::Bool(prior == 0),
    );
    Ok(snap)
}

async fn purchase_order_extra(
    db: &mut PgConnection,
    pd: &PurchaseOrderDoc,
    facts: &Facts,
) -> Result<Facts, ApiError> {
    let po = &pd.row;
    let line_amounts: Vec<Option<f64>> = match facts.get("items") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| match i.get("amount") {
                Some(Value::Number(n)) => n.as_f64(),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    // po.created_at.date(): the timestamp as read (UTC) cut to its date
    let on = po.order_date.or(Some(po.created_at.date_naive()));
    let mut snap = money(
        db,
        f0(po.total),
        po.currency.as_deref(),
        on,
        Some(&line_amounts),
    )
    .await?;
    let quotes: i64 = if let Some(rfq) = po.rfq_id {
        sqlx::query_scalar(
            "SELECT count(DISTINCT vendor_id) FROM purchase_rfq_line_responses WHERE rfq_id = $1",
        )
        .bind(rfq)
        .fetch_one(&mut *db)
        .await?
    } else {
        sqlx::query_scalar("SELECT count(*) FROM attachments WHERE entity_type = 'purchase_order' AND entity_id = $1")
            .bind(po.id.to_string())
            .fetch_one(&mut *db)
            .await?
    };
    snap.insert("vendor_quotes_count".into(), Value::from(quotes));
    Ok(snap)
}

async fn work_order_extra(db: &mut PgConnection, wo: &WorkOrder) -> Result<Facts, ApiError> {
    let mut snap = money(
        db,
        f0(wo.agreed_value),
        wo.currency.as_deref(),
        wo.start_date,
        None,
    )
    .await?;
    let mut request: Option<(Option<String>, Option<Decimal>, Option<String>)> = None;
    if let Some(cr) = wo.contractor_request_id {
        request = sqlx::query_as(
            "SELECT status, estimated_value, currency FROM contractor_requests WHERE id = $1",
        )
        .bind(cr)
        .fetch_optional(&mut *db)
        .await?;
    }
    let approved = request.as_ref().is_some_and(|r| {
        matches!(
            r.0.as_deref(),
            Some("approved" | "in_progress" | "fulfilled")
        )
    });
    let mut above = false;
    if approved {
        let (_, est, cur) = request.as_ref().expect("approved implies a request");
        if let (Some(est), Some(_)) = (est, wo.agreed_value) {
            let (estimate_base, _) = to_base(db, dec_to_f64(*est), cur.as_deref(), None).await?;
            let amount_base = match snap.get("amount_base") {
                Some(Value::Number(n)) => n.as_f64(),
                _ => None,
            };
            above = match (estimate_base, amount_base) {
                (Some(e), Some(a)) => a > e + 0.01,
                _ => true,
            };
        }
    }
    snap.insert(
        "has_approved_contractor_request".into(),
        Value::Bool(approved),
    );
    snap.insert("above_cr_estimate".into(), Value::Bool(above));
    Ok(snap)
}

async fn vendor_invoice_extra(
    db: &mut PgConnection,
    vi: &VendorInvoice,
) -> Result<Facts, ApiError> {
    let mut snap = money(
        db,
        dec_to_f64(vi.amount),
        vi.currency.as_deref(),
        Some(vi.invoice_date),
        None,
    )
    .await?;
    let mut closed = false;
    let mut responsible: Option<Uuid> = None;
    if let Some(wo_id) = vi.work_order_id {
        let wo: Option<(Option<String>, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT status, project_id, created_by_id FROM work_orders WHERE id = $1",
        )
        .bind(wo_id)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((status, project_id, created_by)) = wo {
            closed = status.as_deref() == Some("completed");
            if let Some(pid) = project_id {
                let pm: Option<(Option<Uuid>,)> =
                    sqlx::query_as("SELECT pm_id FROM projects WHERE id = $1")
                        .bind(pid)
                        .fetch_optional(&mut *db)
                        .await?;
                responsible = pm.and_then(|r| r.0);
            }
            responsible = responsible.or(created_by);
        }
    }
    if !closed {
        if let Some(po_id) = vi.purchase_order_id {
            let po: Option<(Option<String>, Uuid)> =
                sqlx::query_as("SELECT status, requested_by_id FROM purchase_orders WHERE id = $1")
                    .bind(po_id)
                    .fetch_optional(&mut *db)
                    .await?;
            if let Some((status, requested_by)) = po {
                closed = status.as_deref() == Some("received");
                responsible = responsible.or(Some(requested_by));
            }
        }
    }
    snap.insert("has_closed_wo_po".into(), Value::Bool(closed));
    snap.insert("responsible_user_id".into(), id_(responsible));
    Ok(snap)
}

/// `routers/manufacturing.py bom_cost(db, bom_items, quantity)`: (sum of unit_cost x quantity per unit x
/// units, functional currency), rounded to its minor unit. Ported here because only this adapter and
/// the manufacturing router use it; the router port should call this one.
pub async fn bom_cost(
    db: &mut PgConnection,
    bom_items: &[BOMItem],
    quantity: Decimal,
) -> ApiResult<(Decimal, String)> {
    let target = crate::fx::functional_currency(db).await?;
    let today = today_local(db).await?;
    let mut rates: Vec<(String, Decimal)> = Vec::new();
    let mut per_unit = Decimal::ZERO;
    for i in bom_items {
        let Some(cost) = i.unit_cost.filter(|c| !c.is_zero()) else {
            continue;
        };
        let cur = truthy(i.cost_currency.as_deref())
            .unwrap_or(&target)
            .to_uppercase();
        let rate = match rates.iter().find(|(c, _)| *c == cur) {
            Some((_, r)) => *r,
            None => {
                let r = crate::fx::require_rate(db, &cur, &target, Some(today)).await?;
                rates.push((cur.clone(), r));
                r
            }
        };
        per_unit += cost * i.quantity * rate;
    }
    let places = currency_places(db, Some(&target)).await?;
    Ok((quantize_money(per_unit * quantity, places), target))
}

/// For routers/manufacturing.py `create_production_order` (the API crate cannot name the row type):
/// `select(BOMItem).where(BOMItem.bom_id == bom_id)` (no ORDER BY), then `bom_cost` when there are items.
/// None when the BOM has no items.
pub async fn bom_cost_of_bom(
    db: &mut PgConnection,
    bom_id: Uuid,
    quantity: Decimal,
) -> ApiResult<Option<(Decimal, String)>> {
    let items =
        fetch_children::<BOMItem>(db, BOMItem::TABLE, BOMItem::COLUMNS, "bom_id", bom_id, "")
            .await?;
    if items.is_empty() {
        return Ok(None);
    }
    Ok(Some(bom_cost(db, &items, quantity).await?))
}

/// `routers/vendor.py INVOICED_STATUSES`.
const INVOICED_STATUSES: &[&str] = &["approved", "partially_paid", "paid"];

/// `routers/vendor.py apply_invoiced_quantities(db, vendor_invoice)`: set PurchaseOrderItem.quantity_invoiced
/// on the invoice's PO, for the three-way match. The invoiced amount is spread over the PO lines pro
/// rata by line amount: each line gets quantity_ordered x (invoiced / PO line total), recomputed from
/// every approved/paid invoice of the PO (plus this one). Ported here because only this adapter and the
/// vendor router use it; the router port should call this one.
///
/// Not identical: `invoiced / lines_total` is rust_decimal division (28-29 significant digits) where
/// Python divides in a 28-digit context; the result is quantized to 3 places, so only a quotient whose
/// 4th decimal sits on a rounding edge 25 digits down could differ.
pub async fn apply_invoiced_quantities(
    db: &mut PgConnection,
    vendor_invoice: &VendorInvoice,
) -> ApiResult<()> {
    let Some(po_id) = vendor_invoice.purchase_order_id else {
        return Ok(());
    };
    let po: Option<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, currency FROM purchase_orders WHERE id = $1 FOR UPDATE")
            .bind(po_id)
            .fetch_optional(&mut *db)
            .await?;
    let Some((po_id, po_currency)) = po else {
        return Ok(());
    };
    let lines: Vec<(Uuid, Decimal, Decimal, Option<Decimal>)> = sqlx::query_as(
        "SELECT id, amount, quantity_ordered, quantity_invoiced FROM purchase_order_items WHERE po_id = $1 \
         ORDER BY id FOR UPDATE",
    )
    .bind(po_id)
    .fetch_all(&mut *db)
    .await?;
    let lines_total: Decimal = lines.iter().map(|l| l.1).sum();
    if lines.is_empty() || lines_total <= Decimal::ZERO {
        return Ok(());
    }
    let statuses: Vec<String> = INVOICED_STATUSES.iter().map(|s| s.to_string()).collect();
    let invoices: Vec<(Decimal, Option<String>, NaiveDate)> = sqlx::query_as(
        "SELECT amount, currency, invoice_date FROM vendor_invoices WHERE purchase_order_id = $1 \
         AND is_deleted = false AND (status = ANY($2) OR id = $3)",
    )
    .bind(po_id)
    .bind(&statuses)
    .bind(vendor_invoice.id)
    .fetch_all(&mut *db)
    .await?;
    let mut invoiced = Decimal::ZERO;
    let po_cur = po_currency.unwrap_or_default().to_uppercase();
    for (amount, currency, invoice_date) in invoices {
        let mut amount = amount;
        let inv_cur = truthy(currency.as_deref())
            .unwrap_or(&po_cur)
            .to_uppercase();
        if inv_cur != po_cur {
            amount *= crate::fx::require_rate(db, &inv_cur, &po_cur, Some(invoice_date)).await?;
        }
        invoiced += amount;
    }
    let share = invoiced / lines_total;
    for (id, _, ordered, current) in lines {
        let value = quantize_money(ordered * share, 3); // Numeric(10, 3)
        if current != Some(value) {
            sqlx::query("UPDATE purchase_order_items SET quantity_invoiced = $1, updated_at = now() WHERE id = $2")
                .bind(value)
                .bind(id)
                .execute(&mut *db)
                .await?;
        }
    }
    Ok(())
}

// ─── SOP publication ─────────────────────────────────────────────────────────

pub const SOP_CONTENT_FIELDS: &[&str] = &[
    "title",
    "content",
    "summary",
    "sop_steps",
    "sop_trigger",
    "sop_owner_role",
    "tags",
    "access_level",
    "is_private",
    "category_id",
];
pub const SOP_SIGNATURE_FIELDS: &[&str] = &[
    "author_signed_at",
    "author_signature_url",
    "owner_signed_by_id",
    "owner_signed_at",
    "owner_signature_url",
];

/// `publish_sop_article(db, article, actor)`: publish an approved, signed SOP. A draft revision of a
/// published SOP (previous_version_id) replaces the published row's content and signatures; the live
/// row keeps its id, slug and code, and the revision row is archived. Returns the row that is now
/// published. Both rows are written at once (`article` is updated in place).
pub async fn publish_sop_article(
    db: &mut PgConnection,
    article: &mut KBArticle,
    actor: Option<&AuthUser>,
) -> Result<KBArticle, ApiError> {
    let actor_id = actor.map(|a| a.id);
    let mut parent: Option<KBArticle> = None;
    if let Some(prev) = article.previous_version_id {
        parent =
            fetch_row::<KBArticle>(db, KBArticle::TABLE, KBArticle::COLUMNS, prev, true).await?;
    }
    let Some(mut parent) = parent else {
        let mut d = Dirty::default();
        set!(d, article, status, Some("published".to_string()));
        set!(
            d,
            article,
            published_by_id,
            actor_id.or(article.published_by_id)
        );
        flush(db, KBArticle::TABLE, article.id, d).await?;
        return Ok(article.clone());
    };

    // str(parent.version) / f"... {article.version}": None prints as "None".
    let py_s = |v: &Option<String>| v.clone().unwrap_or_else(|| "None".to_string());
    sqlx::query(
        "INSERT INTO kb_article_revisions (id, article_id, version, content_snapshot, changed_by_id, change_summary) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(parent.id)
    .bind(py_s(&parent.version))
    .bind(parent.content.clone())
    .bind(actor_id.or(article.created_by_id))
    .bind(format!("Replaced by approved revision {}", py_s(&article.version)))
    .execute(&mut *db)
    .await?;

    let mut d = Dirty::default();
    // SOP_CONTENT_FIELDS + SOP_SIGNATURE_FIELDS
    set!(d, parent, title, article.title.clone());
    set!(d, parent, content, article.content.clone());
    set!(d, parent, summary, article.summary.clone());
    set!(d, parent, sop_steps, article.sop_steps.clone());
    set!(d, parent, sop_trigger, article.sop_trigger.clone());
    set!(d, parent, sop_owner_role, article.sop_owner_role.clone());
    set!(d, parent, tags, article.tags.clone());
    set!(d, parent, access_level, article.access_level.clone());
    set!(d, parent, is_private, article.is_private);
    set!(d, parent, category_id, article.category_id);
    set!(d, parent, author_signed_at, article.author_signed_at);
    set!(
        d,
        parent,
        author_signature_url,
        article.author_signature_url.clone()
    );
    set!(d, parent, owner_signed_by_id, article.owner_signed_by_id);
    set!(d, parent, owner_signed_at, article.owner_signed_at);
    set!(
        d,
        parent,
        owner_signature_url,
        article.owner_signature_url.clone()
    );
    set!(d, parent, version, article.version.clone());
    set!(d, parent, version_notes, article.version_notes.clone());
    set!(d, parent, status, Some("published".to_string()));
    set!(
        d,
        parent,
        published_by_id,
        actor_id.or(parent.published_by_id)
    );
    flush(db, KBArticle::TABLE, parent.id, d).await?;

    let mut d = Dirty::default();
    set!(d, article, status, Some("archived".to_string()));
    flush(db, KBArticle::TABLE, article.id, d).await?;

    // The live row's earlier approval and QR codes describe the old content.
    let reason = format!("Replaced by revision {}", py_s(&article.version));
    crate::approvals::engine::supersede_for_entity(db, "sop_article", parent.id, &reason).await?;
    Ok(parent)
}
