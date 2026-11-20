//! backend/app/routers/p2p.py, mounted at `/api/v1/vendor` (backend/app/main.py), after vendor.py.
//!
//! Procure-to-Pay: RFQ, PO approval, goods receipt, three-way matching. Not behind INTERNAL_ONLY: every
//! route checks `_perm(user, *codes)` (superadmin, or any of the codes; the 403 names the FIRST code)
//! after body validation, except `POST /purchase-orders/{po_id}/approve`, which the approval engine
//! guards.
//!
//! Relationship loads are reproduced with the SQL SQLAlchemy emits (`selectinload`: one query with an
//! expanded IN list, rows in the database's order; `PurchaseRFQ.lines` is `order_by=sort_order`).

use crate::extract::pg_uuid;
use std::collections::HashMap;

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{NaiveDate, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::quantize_money;
use granary_core::settings_store::{currency_places, require_decimal};
use granary_core::validation::{v, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::p2p::{
    PurchaseRFQ, PurchaseRFQLine, PurchaseRFQLineResponse, PurchaseRFQVendorInvite,
};
use granary_db::rows::vendor::{GoodsReceipt, PurchaseOrder, PurchaseOrderItem, VendorInvoice};
use granary_services::approvals::{engine as approvals, notify};

use super::at;
use super::vendor::{one_or_none, rfq_line_ids, upsert_line_response};
use crate::resp::{self, date_iso_opt, dec_float, dec_str, iso, iso_opt, py_float};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/vendor";

/// The routes of `router` in p2p.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/rfqs"), get(list_rfqs).post(create_rfq))
        .route(
            &at(PREFIX, "/rfqs/{rfq_id}"),
            get(get_rfq).delete(delete_rfq),
        )
        .route(&at(PREFIX, "/rfqs/{rfq_id}/lines"), post(add_rfq_line))
        .route(
            &at(PREFIX, "/rfqs/{rfq_id}/lines/{line_id}"),
            axum::routing::delete(delete_rfq_line),
        )
        .route(&at(PREFIX, "/rfqs/{rfq_id}/invite"), post(invite_vendor))
        .route(
            &at(PREFIX, "/rfqs/{rfq_id}/respond"),
            post(submit_vendor_response),
        )
        .route(&at(PREFIX, "/rfqs/{rfq_id}/close"), post(close_rfq))
        .route(
            &at(PREFIX, "/purchase-orders/{po_id}/approve"),
            post(approve_po),
        )
        .route(
            &at(PREFIX, "/goods-receipts"),
            get(list_receipts).post(create_receipt),
        )
        .route(
            &at(PREFIX, "/purchase-orders/{po_id}/matching"),
            get(three_way_match),
        )
}

// ─── Permissions ──────────────────────────────────────────────────────────────

/// `_perm(user, *codes)`.
fn perm(user: &AuthUser, codes: &[&str]) -> ApiResult<()> {
    if user.is_superadmin || codes.iter().any(|c| user.perms.contains(*c)) {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        format!("Permission required: {}", codes[0]),
    ))
}

const VIEW: &[&str] = &["vendor_mgmt:view", "vendor_mgmt:manage_vendors"];
const MANAGE_RFQ: &[&str] = &[
    "vendor_mgmt:manage_rfq",
    "vendor_mgmt:manage_vendors",
    "vendor_mgmt:create_purchase_order",
];

// ─── Serialisers ──────────────────────────────────────────────────────────────

/// `int(x) if x else default` for a Numeric(5, 0).
fn int_if_truthy(x: Option<Decimal>) -> Option<i64> {
    match x {
        Some(d) if !d.is_zero() => Some(d.trunc().to_string().parse().unwrap_or(0)),
        _ => None,
    }
}

/// An RFQ with what `_rfq_dict` reads.
struct RfqFull {
    rfq: PurchaseRFQ,
    invite_count: usize,
    line_count: usize,
    /// Present for `include_lines=True`.
    detail: Option<(Vec<Value>, Vec<Value>)>,
}

/// `_rfq_dict(rfq, include_lines)`.
fn rfq_dict(r: &RfqFull) -> Value {
    let rfq = &r.rfq;
    let mut d = json!({
        "id": rfq.id.to_string(),
        "number": rfq.number,
        "title": rfq.title,
        "description": rfq.description,
        "status": rfq.status,
        "deadline": date_iso_opt(rfq.deadline.as_ref()),
        "currency": rfq.currency,
        "notes": rfq.notes,
        "created_at": iso(&rfq.created_at),
        "vendor_count": r.invite_count,
        "line_count": r.line_count,
    });
    if let Some((lines, invites)) = &r.detail {
        d["lines"] = Value::Array(lines.clone());
        d["vendor_invites"] = Value::Array(invites.clone());
    }
    d
}

/// `_rfq_line_dict(line)` with its responses (each with its vendor's name).
fn rfq_line_dict(
    l: &PurchaseRFQLine,
    responses: &[(PurchaseRFQLineResponse, Option<String>)],
) -> Value {
    json!({
        "id": l.id.to_string(),
        "rfq_id": l.rfq_id.to_string(),
        "sort_order": int_if_truthy(l.sort_order).unwrap_or(0),
        "component_sku": l.component_sku,
        "description": l.description,
        "quantity": dec_float(&l.quantity),
        "unit": l.unit,
        "notes": l.notes,
        "responses": responses.iter().map(|(r, n)| response_dict(r, n.as_deref())).collect::<Vec<_>>(),
    })
}

/// `_invite_dict(inv)`.
fn invite_dict(inv: &PurchaseRFQVendorInvite, vendor_name: Option<&str>) -> Value {
    json!({
        "id": inv.id.to_string(),
        "vendor_id": inv.vendor_id.to_string(),
        "vendor_name": vendor_name,
        "status": inv.status,
        "invited_at": iso_opt(inv.invited_at.as_ref()),
        "responded_at": iso_opt(inv.responded_at.as_ref()),
        "response_notes": inv.response_notes,
    })
}

/// `_response_dict(r)`.
fn response_dict(r: &PurchaseRFQLineResponse, vendor_name: Option<&str>) -> Value {
    json!({
        "id": r.id.to_string(),
        "rfq_line_id": r.rfq_line_id.to_string(),
        "vendor_id": r.vendor_id.to_string(),
        "vendor_name": vendor_name,
        "unit_price": dec_float(&r.unit_price),
        "currency": r.currency,
        "lead_time_days": int_if_truthy(r.lead_time_days),
        "notes": r.notes,
        "is_selected": r.is_selected,
    })
}

/// `_gr_dict(gr, po_number)`.
fn gr_dict(gr: &GoodsReceipt, po_number: Option<&str>) -> Value {
    json!({
        "id": gr.id.to_string(),
        "receipt_number": gr.receipt_number,
        "po_id": gr.po_id.to_string(),
        "po_number": po_number,
        "received_by_id": gr.received_by_id.to_string(),
        "received_at": iso(&gr.received_at),
        "received_date": date_iso_opt(gr.received_date.as_ref()),
        "status": gr.status,
        "location_id": gr.location_id.map(|u| u.to_string()),
        "items_received": match &gr.items_received {
            Some(v) if crate::routers::vendor::truthy(Some(v)) => v.clone(),
            _ => json!([]),
        },
        "notes": gr.notes,
        "created_at": iso(&gr.created_at),
    })
}

/// `$1, $2, ...` for an expanded IN list.
fn marks(n: usize) -> String {
    (1..=n)
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Vendor names by id (`selectinload(...vendor)`, a many-to-one: only the names are read).
async fn vendor_names(db: &mut PgConnection, ids: &[Uuid]) -> ApiResult<HashMap<Uuid, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM vendors WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(&mut *db)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Loads RFQs with their invites and lines: counts only (`selectinload(vendor_invites, lines)`), or the
/// whole tree when `detail` (lines -> responses -> vendor, vendor_invites -> vendor).
async fn load_rfqs(
    db: &mut PgConnection,
    rfqs: Vec<PurchaseRFQ>,
    detail: bool,
) -> ApiResult<Vec<RfqFull>> {
    let ids: Vec<Uuid> = rfqs.iter().map(|r| r.id).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql_inv = format!(
        "SELECT * FROM purchase_rfq_vendor_invites WHERE purchase_rfq_vendor_invites.rfq_id IN ({})",
        marks(ids.len())
    );
    let mut q = sqlx::query_as::<_, PurchaseRFQVendorInvite>(&sql_inv);
    for id in &ids {
        q = q.bind(id);
    }
    let invites = q.fetch_all(&mut *db).await?;
    let sql_lines = format!(
        "SELECT * FROM purchase_rfq_lines WHERE purchase_rfq_lines.rfq_id IN ({}) \
         ORDER BY purchase_rfq_lines.sort_order",
        marks(ids.len())
    );
    let mut q = sqlx::query_as::<_, PurchaseRFQLine>(&sql_lines);
    for id in &ids {
        q = q.bind(id);
    }
    let lines = q.fetch_all(&mut *db).await?;

    let mut responses: Vec<PurchaseRFQLineResponse> = Vec::new();
    let mut names = HashMap::new();
    if detail {
        if !lines.is_empty() {
            let sql = format!(
                "SELECT * FROM purchase_rfq_line_responses \
                 WHERE purchase_rfq_line_responses.rfq_line_id IN ({})",
                marks(lines.len())
            );
            let mut q = sqlx::query_as::<_, PurchaseRFQLineResponse>(&sql);
            for l in &lines {
                q = q.bind(l.id);
            }
            responses = q.fetch_all(&mut *db).await?;
        }
        let mut vids: Vec<Uuid> = responses.iter().map(|r| r.vendor_id).collect();
        vids.extend(invites.iter().map(|i| i.vendor_id));
        names = vendor_names(db, &vids).await?;
    }
    let mut out = Vec::with_capacity(rfqs.len());
    for rfq in rfqs {
        let my_invites: Vec<&PurchaseRFQVendorInvite> =
            invites.iter().filter(|i| i.rfq_id == rfq.id).collect();
        let my_lines: Vec<&PurchaseRFQLine> = lines.iter().filter(|l| l.rfq_id == rfq.id).collect();
        let detail_v = if detail {
            let line_values: Vec<Value> = my_lines
                .iter()
                .map(|l| {
                    let rs: Vec<(PurchaseRFQLineResponse, Option<String>)> = responses
                        .iter()
                        .filter(|r| r.rfq_line_id == l.id)
                        .map(|r| (r.clone(), names.get(&r.vendor_id).cloned()))
                        .collect();
                    rfq_line_dict(l, &rs)
                })
                .collect();
            let invite_values: Vec<Value> = my_invites
                .iter()
                .map(|i| invite_dict(i, names.get(&i.vendor_id).map(String::as_str)))
                .collect();
            Some((line_values, invite_values))
        } else {
            None
        };
        out.push(RfqFull {
            invite_count: my_invites.len(),
            line_count: my_lines.len(),
            detail: detail_v,
            rfq,
        });
    }
    Ok(out)
}

async fn rfq_detail(db: &mut PgConnection, rfq_id: Uuid) -> ApiResult<Option<Value>> {
    let rfq: Option<PurchaseRFQ> =
        sqlx::query_as("SELECT * FROM purchase_rfqs WHERE id = $1 AND is_deleted = false")
            .bind(rfq_id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(rfq) = rfq else { return Ok(None) };
    let full = load_rfqs(db, vec![rfq], true).await?;
    Ok(full.first().map(rfq_dict))
}

// ─── RFQ CRUD ─────────────────────────────────────────────────────────────────

/// `PurchaseRFQLine.quantity` is Numeric(10, 3).
const QTY_MAX: &str = "9999999";

granary_core::py_model! {
    /// `class RFQLineInput(BaseModel)`.
    pub struct RFQLineInput {
        pub description: String,
        pub quantity: Decimal = Decimal::ONE => v::decimal_c(v::DecC::new().gt("0").le(QTY_MAX)),
        pub unit: String = "pcs".to_string(),
        pub component_sku: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class RFQCreate(BaseModel)`.
    pub struct RFQCreate {
        pub title: String,
        pub description: Option<String> = None,
        pub deadline: Option<NaiveDate> = None,
        pub currency: String = String::new(),
        pub notes: Option<String> = None,
        pub lines: Vec<RFQLineInput> = Vec::new(),
    }
}

granary_core::py_model! {
    /// `GET /rfqs` query.
    pub struct ListRfqsQ {
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /rfqs`.
pub async fn list_rfqs(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListRfqsQ>,
) -> ApiResult<Response> {
    perm(&user, VIEW)?;
    let mut conn = db.conn().await?;
    let status = q.status.filter(|s| !s.is_empty());
    let rfqs: Vec<PurchaseRFQ> = sqlx::query_as(
        "SELECT * FROM purchase_rfqs WHERE is_deleted = false AND ($1::text IS NULL OR status = $1) \
         ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(status)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let full = load_rfqs(&mut conn, rfqs, false).await?;
    Ok(resp::json(Value::Array(
        full.iter().map(rfq_dict).collect(),
    )))
}

/// `POST /rfqs` (201).
pub async fn create_rfq(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<RFQCreate>,
) -> ApiResult<Response> {
    perm(&user, MANAGE_RFQ)?;
    let mut conn = db.conn().await?;
    // No currency given: the company's functional currency (not a fixed one).
    let mut body = body;
    if body.currency.trim().is_empty() {
        body.currency = granary_services::fx::functional_currency(&mut conn).await?;
    }
    let number = granary_services::numbering::next_document_number(&mut conn, "rfq").await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_rfqs (number, title, description, deadline, currency, notes, created_by_id, \
         status, id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, 'draft', $8, false)",
    )
    .bind(&number)
    .bind(&body.title)
    .bind(&body.description)
    .bind(body.deadline)
    .bind(&body.currency)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    for (i, line) in body.lines.iter().enumerate() {
        insert_rfq_line(
            &mut conn,
            id,
            i as i64,
            line.component_sku.as_deref(),
            &line.description,
            line.quantity,
            &line.unit,
            line.notes.as_deref(),
        )
        .await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let out = rfq_detail(&mut conn, id).await?.ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("No row was found when one was required"))
    })?;
    Ok(resp::json_status(201, out))
}

#[allow(clippy::too_many_arguments)]
async fn insert_rfq_line(
    db: &mut PgConnection,
    rfq_id: Uuid,
    sort_order: i64,
    component_sku: Option<&str>,
    description: &str,
    quantity: Decimal,
    unit: &str,
    notes: Option<&str>,
) -> ApiResult<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_rfq_lines (rfq_id, sort_order, component_sku, description, quantity, unit, \
         notes, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(rfq_id)
    .bind(Decimal::from(sort_order))
    .bind(component_sku)
    .bind(description)
    .bind(quantize_money(quantity, 3))
    .bind(unit)
    .bind(notes)
    .bind(id)
    .execute(&mut *db)
    .await?;
    Ok(id)
}

/// `GET /rfqs/{rfq_id}`.
pub async fn get_rfq(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rfq_id): Path<String>,
) -> ApiResult<Response> {
    perm(&user, VIEW)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&rfq_id)?;
    match rfq_detail(&mut conn, id).await? {
        Some(v) => Ok(resp::json(v)),
        None => Err(ApiError::http(404, "RFQ not found")),
    }
}

async fn open_rfq(
    db: &mut PgConnection,
    rfq_id: &str,
    for_update: bool,
) -> ApiResult<Option<PurchaseRFQ>> {
    let id = pg_uuid(rfq_id)?;
    let sql = if for_update {
        "SELECT * FROM purchase_rfqs WHERE id = $1 AND is_deleted = false FOR UPDATE"
    } else {
        "SELECT * FROM purchase_rfqs WHERE id = $1 AND is_deleted = false"
    };
    Ok(sqlx::query_as(sql)
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// `DELETE /rfqs/{rfq_id}` (204).
pub async fn delete_rfq(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rfq_id): Path<String>,
) -> ApiResult<Response> {
    perm(&user, MANAGE_RFQ)?;
    let mut conn = db.conn().await?;
    let Some(rfq) = open_rfq(&mut conn, &rfq_id, false).await? else {
        return Err(ApiError::http(404, "RFQ not found"));
    };
    if !matches!(rfq.status.as_deref(), Some("draft" | "cancelled")) {
        return Err(ApiError::http(
            400,
            "Only draft or cancelled RFQs can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE purchase_rfqs SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(rfq.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── RFQ Lines ────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class LineAdd(BaseModel)`.
    pub struct LineAdd {
        pub component_sku: Option<String> = None,
        pub description: String,
        pub quantity: Decimal => v::decimal_c(v::DecC::new().gt("0").le(QTY_MAX)),
        pub unit: String = "pcs".to_string(),
        pub notes: Option<String> = None,
    }
}

/// `POST /rfqs/{rfq_id}/lines` (201).
pub async fn add_rfq_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rfq_id): Path<String>,
    PyJson(body): PyJson<LineAdd>,
) -> ApiResult<Response> {
    perm(&user, MANAGE_RFQ)?;
    let mut conn = db.conn().await?;
    let Some(rfq) = open_rfq(&mut conn, &rfq_id, false).await? else {
        return Err(ApiError::http(404, "RFQ not found"));
    };
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM purchase_rfq_lines WHERE rfq_id = $1")
            .bind(rfq.id)
            .fetch_one(&mut *conn)
            .await?;
    let line_id = insert_rfq_line(
        &mut conn,
        rfq.id,
        count,
        body.component_sku.as_deref(),
        &body.description,
        body.quantity,
        &body.unit,
        body.notes.as_deref(),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    // Reloaded with its responses (a new line has none).
    let mut conn = db.conn().await?;
    let line: PurchaseRFQLine = sqlx::query_as("SELECT * FROM purchase_rfq_lines WHERE id = $1")
        .bind(line_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json_status(201, rfq_line_dict(&line, &[])))
}

/// `DELETE /rfqs/{rfq_id}/lines/{line_id}` (204).
pub async fn delete_rfq_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((rfq_id, line_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    perm(&user, MANAGE_RFQ)?;
    let mut conn = db.conn().await?;
    let lid = pg_uuid(&line_id)?;
    let rid = pg_uuid(&rfq_id)?;
    let found: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM purchase_rfq_lines WHERE id = $1 AND rfq_id = $2")
            .bind(lid)
            .bind(rid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(id) = found else {
        return Err(ApiError::http(404, "Line not found"));
    };
    // cascade="all, delete-orphan": the ORM deletes the line's responses first.
    sqlx::query("DELETE FROM purchase_rfq_line_responses WHERE rfq_line_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM purchase_rfq_lines WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Vendor invitations ───────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class InviteVendor(BaseModel)`.
    pub struct InviteVendor {
        pub vendor_id: String,
    }
}

/// `POST /rfqs/{rfq_id}/invite`.
pub async fn invite_vendor(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rfq_id): Path<String>,
    PyJson(body): PyJson<InviteVendor>,
) -> ApiResult<Response> {
    perm(&user, MANAGE_RFQ)?;
    let mut conn = db.conn().await?;
    let Some(rfq) = open_rfq(&mut conn, &rfq_id, false).await? else {
        return Err(ApiError::http(404, "RFQ not found"));
    };
    let vendor_id = pg_uuid(&body.vendor_id)?;
    let existing: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM purchase_rfq_vendor_invites WHERE rfq_id = $1 AND vendor_id = $2 LIMIT 2",
    )
    .bind(rfq.id)
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    if one_or_none(existing)?.is_some() {
        return Err(ApiError::http(409, "Vendor already invited"));
    }
    let invite_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_rfq_vendor_invites (rfq_id, vendor_id, status, invited_at, id) \
         VALUES ($1, $2, 'invited', $3, $4)",
    )
    .bind(rfq.id)
    .bind(vendor_id)
    .bind(Utc::now())
    .bind(invite_id)
    .execute(&mut *conn)
    .await?;
    // Advance RFQ to sent if it was draft
    if rfq.status.as_deref() == Some("draft") {
        sqlx::query("UPDATE purchase_rfqs SET status = 'sent', updated_at = now() WHERE id = $1")
            .bind(rfq.id)
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"status": "invited", "invite_id": invite_id.to_string()}),
    ))
}

// ─── Vendor responses ─────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class VendorResponseLine(BaseModel)` (a client-sent currency is ignored).
    pub struct VendorResponseLine {
        pub rfq_line_id: String,
        pub unit_price: Decimal => v::decimal_c(v::DecC::new().gt("0").le("99999999999999")),
        pub lead_time_days: Option<i64> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class VendorResponse(BaseModel)`.
    pub struct VendorResponse {
        pub vendor_id: String,
        pub response_notes: Option<String> = None,
        pub line_responses: Vec<VendorResponseLine>,
    }
}

/// `POST /rfqs/{rfq_id}/respond`: record a quote on a vendor's behalf (needs the rights to run the RFQ).
pub async fn submit_vendor_response(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rfq_id): Path<String>,
    PyJson(body): PyJson<VendorResponse>,
) -> ApiResult<Response> {
    perm(&user, MANAGE_RFQ)?;
    let mut conn = db.conn().await?;
    let Some(rfq) = open_rfq(&mut conn, &rfq_id, true).await? else {
        return Err(ApiError::http(404, "RFQ not found"));
    };
    if let Some(s @ ("closed" | "cancelled")) = rfq.status.as_deref() {
        return Err(ApiError::http(
            400,
            format!("This RFQ is {s} and no longer accepts quotes"),
        ));
    }
    let vendor_id = pg_uuid(&body.vendor_id)?;
    let invite: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM purchase_rfq_vendor_invites WHERE rfq_id = $1 AND vendor_id = $2 LIMIT 2",
    )
    .bind(rfq.id)
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let Some((invite_id,)) = one_or_none(invite)? else {
        return Err(ApiError::http(
            400,
            "This vendor was not invited to the RFQ",
        ));
    };
    let valid_lines = rfq_line_ids(&mut conn, rfq.id).await?;
    sqlx::query(
        "UPDATE purchase_rfq_vendor_invites SET status = 'responded', responded_at = $2, response_notes = $3, \
         updated_at = now() WHERE id = $1",
    )
    .bind(invite_id)
    .bind(Utc::now())
    .bind(&body.response_notes)
    .execute(&mut *conn)
    .await?;
    for lr in &body.line_responses {
        if !valid_lines.contains(&lr.rfq_line_id) {
            return Err(ApiError::http(
                400,
                format!("Line {} does not belong to this RFQ", lr.rfq_line_id),
            ));
        }
        upsert_line_response(
            &mut conn,
            rfq.id,
            &lr.rfq_line_id,
            vendor_id,
            lr.unit_price,
            rfq.currency.as_deref(),
            lr.lead_time_days,
            lr.notes.as_deref(),
        )
        .await?;
    }
    if rfq.status.as_deref() == Some("sent") {
        sqlx::query(
            "UPDATE purchase_rfqs SET status = 'evaluating', updated_at = now() WHERE id = $1",
        )
        .bind(rfq.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "response_recorded"})))
}

// ─── Close RFQ -> create PO ───────────────────────────────────────────────────

granary_core::py_model! {
    /// `class CloseRFQ(BaseModel)`.
    pub struct CloseRFQ {
        pub create_po: bool = true,
        pub vendor_id: String,
        pub selected_line_responses: Vec<String>,
        pub currency: Option<String> = None,
        pub expected_delivery: Option<NaiveDate> = None,
        pub shipping_address: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

/// `POST /rfqs/{rfq_id}/close`: close an RFQ and optionally create a PO from the selected responses.
pub async fn close_rfq(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rfq_id): Path<String>,
    PyJson(body): PyJson<CloseRFQ>,
) -> ApiResult<Response> {
    perm(&user, MANAGE_RFQ)?;
    let mut conn = db.conn().await?;
    // Lock the RFQ so two concurrent closes can't both create a PO.
    let Some(rfq) = open_rfq(&mut conn, &rfq_id, true).await? else {
        return Err(ApiError::http(404, "RFQ not found"));
    };
    let lines: Vec<PurchaseRFQLine> = sqlx::query_as(
        "SELECT * FROM purchase_rfq_lines WHERE purchase_rfq_lines.rfq_id IN ($1) \
         ORDER BY purchase_rfq_lines.sort_order",
    )
    .bind(rfq.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut responses: Vec<PurchaseRFQLineResponse> = Vec::new();
    if !lines.is_empty() {
        let sql = format!(
            "SELECT * FROM purchase_rfq_line_responses WHERE purchase_rfq_line_responses.rfq_line_id IN ({})",
            marks(lines.len())
        );
        let mut q = sqlx::query_as::<_, PurchaseRFQLineResponse>(&sql);
        for l in &lines {
            q = q.bind(l.id);
        }
        responses = q.fetch_all(&mut *conn).await?;
    }
    let status = rfq.status.clone().unwrap_or_default();
    if !matches!(status.as_str(), "sent" | "evaluating") {
        return Err(ApiError::http(
            409,
            format!(
                "Only a sent or evaluating RFQ can be closed (this one is {})",
                rfq.status.as_deref().unwrap_or("None")
            ),
        ));
    }

    // Mark selected responses
    let selected: std::collections::HashSet<&str> = body
        .selected_line_responses
        .iter()
        .map(String::as_str)
        .collect();
    for r in &responses {
        let is_sel = selected.contains(r.id.to_string().as_str());
        if r.is_selected != Some(is_sel) {
            sqlx::query(
                "UPDATE purchase_rfq_line_responses SET is_selected = $2, updated_at = now() WHERE id = $1",
            )
            .bind(r.id)
            .bind(is_sel)
            .execute(&mut *conn)
            .await?;
        }
    }
    sqlx::query("UPDATE purchase_rfqs SET status = 'closed', updated_at = now() WHERE id = $1")
        .bind(rfq.id)
        .execute(&mut *conn)
        .await?;
    let mut po_id: Option<String> = None;

    if body.create_po && !body.selected_line_responses.is_empty() {
        // Build PO lines from selected responses, all in the PO's currency
        let currency = match body.currency.as_deref().filter(|s| !s.is_empty()) {
            Some(c) => c.to_string(),
            None => rfq.currency.clone().ok_or_else(|| {
                ApiError::internal(anyhow::anyhow!(
                    "AttributeError: 'NoneType' object has no attribute 'upper'"
                ))
            })?,
        };
        let mut chosen: Vec<(&PurchaseRFQLine, &PurchaseRFQLineResponse)> = Vec::new();
        for line in &lines {
            let selected_resp = responses.iter().find(|r| {
                r.rfq_line_id == line.id
                    && selected.contains(r.id.to_string().as_str())
                    && r.vendor_id.to_string() == body.vendor_id
            });
            let Some(sr) = selected_resp else { continue };
            if sr.currency.clone().unwrap_or_default().to_uppercase() != currency.to_uppercase() {
                return Err(ApiError::http(
                    400,
                    format!(
                        "The quote for '{}' is in {}, not the PO currency {currency}",
                        line.description,
                        sr.currency.as_deref().unwrap_or("None")
                    ),
                ));
            }
            chosen.push((line, sr));
        }
        if chosen.is_empty() {
            return Err(ApiError::http(
                400,
                "None of the selected responses are from this vendor",
            ));
        }
        let places = currency_places(&mut conn, Some(&currency)).await?;
        let number =
            granary_services::numbering::next_document_number(&mut conn, "purchase_order").await?;
        let order_date = granary_core::clock::today_local(&mut conn).await?;
        let vendor_id = pg_uuid(&body.vendor_id)?;
        let new_id = Uuid::new_v4();
        let mut subtotal = Decimal::ZERO;
        let mut items = Vec::new();
        for (line, sr) in &chosen {
            let amount = quantize_money(line.quantity * sr.unit_price, places);
            subtotal += amount;
            items.push((*line, *sr, amount));
        }
        sqlx::query(
            "INSERT INTO purchase_orders (number, vendor_id, rfq_id, status, approval_status, currency, \
             order_date, expected_delivery, shipping_address, notes, requested_by_id, created_by_id, \
             subtotal, tax_amount, shipping_cost, total, id, is_deleted) VALUES ($1, $2, $3, 'draft', \
             'not_required', $4, $5, $6, $7, $8, $9, $9, $10, 0, 0, $10, $11, false)",
        )
        .bind(&number)
        .bind(vendor_id)
        .bind(rfq.id)
        .bind(&currency)
        .bind(order_date)
        .bind(body.expected_delivery)
        .bind(&body.shipping_address)
        .bind(&body.notes)
        .bind(user.id)
        .bind(subtotal)
        .bind(new_id)
        .execute(&mut *conn)
        .await?;
        for (line, sr, amount) in items {
            sqlx::query(
                "INSERT INTO purchase_order_items (po_id, rfq_line_id, component_sku, description, \
                 quantity_ordered, unit, unit_price, currency, amount, quantity_received, quantity_invoiced, id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 0, 0, $10)",
            )
            .bind(new_id)
            .bind(line.id)
            .bind(&line.component_sku)
            .bind(&line.description)
            .bind(line.quantity)
            .bind(&line.unit)
            .bind(sr.unit_price)
            .bind(&currency)
            .bind(amount)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
        po_id = Some(new_id.to_string());
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"status": "closed", "purchase_order_id": po_id}),
    ))
}

// ─── PO approval ──────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class ApprovalAction(BaseModel)`.
    pub struct ApprovalAction {
        pub action: String,
        pub notes: Option<String> = None,
        /// unused; kept for old clients
        pub amount_threshold: Decimal = Decimal::ZERO => v::decimal_c(v::DecC::new().ge("0")),
    }
}

/// `POST /purchase-orders/{po_id}/approve`: decide the PO's current approval step. On final
/// approval a draft PO is released as sent; a rejection leaves it in draft for rework.
pub async fn approve_po(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(po_id): Path<String>,
    PyJson(body): PyJson<ApprovalAction>,
) -> ApiResult<Response> {
    let _ = body.amount_threshold;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&po_id)?;
    let po: Option<PurchaseOrder> =
        sqlx::query_as("SELECT * FROM purchase_orders WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(po) = po else {
        return Err(ApiError::http(404, "Purchase order not found"));
    };
    if !matches!(body.action.as_str(), "approve" | "reject") {
        return Err(ApiError::http(400, "action must be 'approve' or 'reject'"));
    }
    let decision = if body.action == "approve" {
        "approved"
    } else {
        "rejected"
    };
    let request = approvals::act_on_pending(
        &mut conn,
        "purchase_order",
        po.id,
        &user,
        decision,
        body.notes.as_deref(),
    )
    .await?;
    let (mut po_status, approval_status): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT status, approval_status FROM purchase_orders WHERE id = $1")
            .bind(po.id)
            .fetch_one(&mut *conn)
            .await?;
    if request.status == "approved" && po_status.as_deref() == Some("draft") {
        sqlx::query("UPDATE purchase_orders SET status = 'sent', updated_at = now() WHERE id = $1")
            .bind(po.id)
            .execute(&mut *conn)
            .await?;
        po_status = Some("sent".into());
    }
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json(json!({
        "approval_status": approval_status, "po_status": po_status,
        "request_status": request.status, "current_step_seq": request.current_step_seq,
    })))
}

// ─── Goods Receipt ────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class GoodsReceiptItem(BaseModel)` (a client-sent sku is ignored).
    pub struct GoodsReceiptItem {
        pub po_item_id: String,
        pub qty_received: Decimal => v::decimal_c(v::DecC::new().gt("0")),
        pub condition: String = "good".to_string(),
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class GoodsReceiptCreate(BaseModel)`.
    pub struct GoodsReceiptCreate {
        pub po_id: String,
        pub received_date: Option<NaiveDate> = None,
        pub location_id: Option<String> = None,
        pub notes: Option<String> = None,
        pub items: Vec<GoodsReceiptItem> => v::list_c(v::ListC::new().min_length(1)),
    }
}

const RECEIVABLE_PO_STATUSES: &[&str] = &["sent", "confirmed", "partially_received"];
const RECEIPT_TOLERANCE_KEY: &str = "procurement.receipt_over_tolerance_pct";

granary_core::py_model! {
    /// `GET /goods-receipts` query.
    pub struct ListReceiptsQ {
        pub po_id: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /goods-receipts`.
pub async fn list_receipts(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListReceiptsQ>,
) -> ApiResult<Response> {
    perm(&user, VIEW)?;
    let mut conn = db.conn().await?;
    let po_id = match q.po_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let rows = sqlx::query(
        "SELECT goods_receipts.*, purchase_orders.number AS po_number FROM goods_receipts \
         JOIN purchase_orders ON goods_receipts.po_id = purchase_orders.id \
         WHERE ($1::uuid IS NULL OR goods_receipts.po_id = $1) \
         ORDER BY goods_receipts.received_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(po_id)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let gr = <GoodsReceipt as sqlx::FromRow<_>>::from_row(r)?;
        let number: String = r.get("po_number");
        out.push(gr_dict(&gr, Some(&number)));
    }
    Ok(resp::json(Value::Array(out)))
}

/// A `ComponentInventory` row as `create_receipt` reads and writes it.
struct InventoryRow {
    id: Uuid,
    component_name: String,
    on_hand: Decimal,
    reserved: Decimal,
    available: Decimal,
    location_id: Option<Uuid>,
    location_set: Option<Uuid>,
    touched: bool,
}

/// `POST /goods-receipts` (201): record goods received against a PO (PO line quantities, component
/// inventory, PO status).
pub async fn create_receipt(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<GoodsReceiptCreate>,
) -> ApiResult<Response> {
    perm(
        &user,
        &[
            "vendor_mgmt:manage_vendors",
            "vendor_mgmt:approve_purchase_order",
        ],
    )?;
    let mut conn = db.conn().await?;
    // Lock the PO and its lines so concurrent receipts can't both pass the over-receipt check.
    let pid = pg_uuid(&body.po_id)?;
    let po: Option<PurchaseOrder> = sqlx::query_as(
        "SELECT * FROM purchase_orders WHERE id = $1 AND is_deleted = false FOR UPDATE",
    )
    .bind(pid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(po) = po else {
        return Err(ApiError::http(404, "Purchase order not found"));
    };
    let po_status = po.status.clone().unwrap_or_default();
    if !RECEIVABLE_PO_STATUSES.contains(&po_status.as_str()) {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot receive against a {} PO (allowed: {})",
                po.status.as_deref().unwrap_or("None"),
                RECEIVABLE_PO_STATUSES.join(", ")
            ),
        ));
    }
    if po.approval_status.as_deref() == Some("pending") {
        return Err(ApiError::http(
            400,
            "Cannot receive against a PO whose approval is pending",
        ));
    }
    if po.approval_status.as_deref() != Some("approved") {
        // Goods are received only against an approved PO. One that skipped approval is
        // submitted now; receiving waits for the approver (409).
        approvals::ensure_or_submit(&mut conn, "purchase_order", po.id, &user, "received", None)
            .await?;
    }
    let mut po_items: Vec<PurchaseOrderItem> = sqlx::query_as(
        "SELECT * FROM purchase_order_items WHERE po_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(po.id)
    .fetch_all(&mut *conn)
    .await?;
    let index: HashMap<String, usize> = po_items
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.to_string(), i))
        .collect();
    for item in &body.items {
        if !index.contains_key(&item.po_item_id) {
            return Err(ApiError::http(
                400,
                format!("Line {} is not on this PO", item.po_item_id),
            ));
        }
    }

    let tolerance_pct = require_decimal(
        &mut conn,
        RECEIPT_TOLERANCE_KEY,
        "Operations \u{2192} Procurement",
    )
    .await?;
    let over_factor = Decimal::ONE + tolerance_pct / Decimal::from(100);

    // Inventory rows for the received SKUs, locked in a fixed order.
    let mut skus: Vec<String> = body
        .items
        .iter()
        .filter_map(|i| po_items[index[&i.po_item_id]].component_sku.clone())
        .filter(|s| !s.is_empty())
        .collect();
    skus.sort();
    skus.dedup();
    let mut inventory: HashMap<String, InventoryRow> = HashMap::new();
    if !skus.is_empty() {
        let rows = sqlx::query(
            "SELECT id, component_sku, component_name, quantity_on_hand, quantity_reserved, \
             quantity_available, location_id FROM component_inventory WHERE component_sku = ANY($1) \
             ORDER BY component_sku FOR UPDATE",
        )
        .bind(&skus)
        .fetch_all(&mut *conn)
        .await?;
        for r in rows {
            inventory.insert(
                r.get("component_sku"),
                InventoryRow {
                    id: r.get("id"),
                    component_name: r.get("component_name"),
                    on_hand: r
                        .get::<Option<Decimal>, _>("quantity_on_hand")
                        .unwrap_or_default(),
                    reserved: r
                        .get::<Option<Decimal>, _>("quantity_reserved")
                        .unwrap_or_default(),
                    available: r
                        .get::<Option<Decimal>, _>("quantity_available")
                        .unwrap_or_default(),
                    location_id: r.get("location_id"),
                    location_set: None,
                    touched: false,
                },
            );
        }
    }

    let now = Utc::now();
    let receipt_number =
        granary_services::numbering::next_document_number(&mut conn, "goods_receipt").await?;
    let location = match body.location_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };

    let mut items_json = Vec::new();
    let mut moves = Vec::new();
    for item in &body.items {
        let po_item = &mut po_items[index[&item.po_item_id]];
        let qty = item.qty_received;
        let sku = po_item.component_sku.clone();
        let received = po_item.quantity_received.unwrap_or_default() + qty;
        let limit = po_item.quantity_ordered * over_factor;
        if received > limit {
            return Err(ApiError::http(
                400,
                format!(
                    "Receiving {} of '{}' would bring it to {} against {} ordered (tolerance {}%)",
                    dec_str(&qty),
                    po_item.description,
                    dec_str(&received),
                    dec_str(&po_item.quantity_ordered),
                    dec_str(&tolerance_pct)
                ),
            ));
        }
        po_item.quantity_received = Some(received);

        if let Some(sku) = sku.as_deref().filter(|s| !s.is_empty()) {
            let inv = inventory.get_mut(sku);
            let component_name = match inv {
                Some(inv) => {
                    inv.on_hand += qty;
                    inv.available = (inv.on_hand - inv.reserved).max(Decimal::ZERO);
                    if location.is_some() && inv.location_id.is_none() && inv.location_set.is_none()
                    {
                        inv.location_set = location;
                    }
                    inv.touched = true;
                    inv.component_name.clone()
                }
                None => po_item.description.clone(),
            };
            moves.push((sku.to_string(), component_name, qty));
        }
        items_json.push(json!({
            "sku": sku,
            "po_item_id": item.po_item_id,
            "qty_received": py_float(dec_to_f64(&qty)),
            "condition": item.condition,
            "notes": item.notes,
        }));
    }

    for it in &po_items {
        if body.items.iter().any(|b| b.po_item_id == it.id.to_string()) {
            sqlx::query(
                "UPDATE purchase_order_items SET quantity_received = $2, updated_at = now() WHERE id = $1",
            )
            .bind(it.id)
            .bind(it.quantity_received)
            .execute(&mut *conn)
            .await?;
        }
    }
    for inv in inventory.values().filter(|i| i.touched) {
        sqlx::query(
            "UPDATE component_inventory SET quantity_on_hand = $2, quantity_available = $3, \
             location_id = COALESCE($4, location_id), updated_by_id = $5, updated_at = now() WHERE id = $1",
        )
        .bind(inv.id)
        .bind(inv.on_hand)
        .bind(inv.available)
        .bind(inv.location_set)
        .bind(user.id)
        .execute(&mut *conn)
        .await?;
    }
    for (sku, name, qty) in moves {
        sqlx::query(
            "INSERT INTO stock_moves (component_sku, component_name, to_location_id, quantity, move_type, \
             reference, notes, moved_by_id, moved_at, id) VALUES ($1, $2, $3, $4, 'receipt', $5, $6, $7, $8, $9)",
        )
        .bind(&sku)
        .bind(&name)
        .bind(location)
        .bind(qty)
        .bind(&po.number)
        .bind(format!("Goods receipt {receipt_number}"))
        .bind(user.id)
        .bind(now)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }

    // Determine PO status
    let all_received = po_items
        .iter()
        .all(|i| i.quantity_received.unwrap_or_default() >= i.quantity_ordered);
    let any_received = po_items
        .iter()
        .any(|i| i.quantity_received.unwrap_or_default() > Decimal::ZERO);
    if all_received {
        let today = granary_core::clock::today_local(&mut conn).await?;
        sqlx::query(
            "UPDATE purchase_orders SET status = 'received', actual_delivery = $2, updated_at = now() WHERE id = $1",
        )
        .bind(po.id)
        .bind(today)
        .execute(&mut *conn)
        .await?;
    } else if any_received && po.status.as_deref() != Some("partially_received") {
        sqlx::query(
            "UPDATE purchase_orders SET status = 'partially_received', updated_at = now() WHERE id = $1",
        )
        .bind(po.id)
        .execute(&mut *conn)
        .await?;
    }

    let received_date = match body.received_date {
        Some(d) => d,
        None => granary_core::clock::today_local(&mut conn).await?,
    };
    let gr_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO goods_receipts (receipt_number, po_id, received_by_id, received_at, received_date, \
         status, location_id, items_received, notes, id) VALUES ($1, $2, $3, $4, $5, 'confirmed', $6, $7, $8, $9)",
    )
    .bind(&receipt_number)
    .bind(po.id)
    .bind(user.id)
    .bind(now)
    .bind(received_date)
    .bind(location)
    .bind(Value::Array(items_json))
    .bind(&body.notes)
    .bind(gr_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let gr: GoodsReceipt = sqlx::query_as("SELECT * FROM goods_receipts WHERE id = $1")
        .bind(gr_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json_status(201, gr_dict(&gr, Some(&po.number))))
}

/// `float(Decimal)`.
fn dec_to_f64(d: &Decimal) -> f64 {
    d.to_string().parse().unwrap_or(f64::NAN)
}

// ─── 3-way matching ───────────────────────────────────────────────────────────

/// VendorInvoice statuses that count as invoiced (see vendor.apply_invoiced_quantities).
const INVOICED_STATUSES: &[&str] = &["approved", "partially_paid", "paid"];

/// `round(x / ordered * 100, 1)` (Decimal: ROUND_HALF_EVEN).
fn pct(x: Decimal, ordered: Decimal) -> Value {
    if ordered.is_zero() {
        return json!(0);
    }
    let r = (x / ordered * Decimal::from(100))
        .round_dp_with_strategy(1, RoundingStrategy::MidpointNearestEven);
    dec_float(&r)
}

/// `GET /purchase-orders/{po_id}/matching`: ordered vs received vs invoiced per PO line, plus an overall
/// match status. A line is "matched" only when received and invoiced both equal the ordered quantity; more
/// received or invoiced than ordered (the receipt tolerance allows a little over) is "over", flagged per line
/// (`over_received`, `over_invoiced`) and overall ("over").
pub async fn three_way_match(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(po_id): Path<String>,
) -> ApiResult<Response> {
    perm(&user, VIEW)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&po_id)?;
    let po: Option<PurchaseOrder> =
        sqlx::query_as("SELECT * FROM purchase_orders WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(po) = po else {
        return Err(ApiError::http(404, "Purchase order not found"));
    };
    // selectinload(PurchaseOrder.items), selectinload(PurchaseOrder.receipts): no order_by.
    let items: Vec<PurchaseOrderItem> = sqlx::query_as(
        "SELECT * FROM purchase_order_items WHERE purchase_order_items.po_id IN ($1)",
    )
    .bind(po.id)
    .fetch_all(&mut *conn)
    .await?;
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM goods_receipts WHERE po_id = $1")
            .bind(po.id)
            .fetch_one(&mut *conn)
            .await?;
    // Invoices that count: approved ones (and their payments).
    let invoices: Vec<VendorInvoice> = sqlx::query_as(
        "SELECT * FROM vendor_invoices WHERE purchase_order_id = $1 AND is_deleted = false \
         AND status = ANY($2)",
    )
    .bind(po.id)
    .bind(INVOICED_STATUSES)
    .fetch_all(&mut *conn)
    .await?;
    let mut invoiced_by_currency: Vec<(String, Decimal)> = Vec::new();
    for inv in &invoices {
        let cur = inv
            .currency
            .clone()
            .filter(|c| !c.is_empty())
            .or_else(|| po.currency.clone())
            .unwrap_or_default()
            .to_uppercase();
        match invoiced_by_currency.iter_mut().find(|(c, _)| *c == cur) {
            Some((_, amt)) => *amt += inv.amount,
            None => invoiced_by_currency.push((cur, inv.amount)),
        }
    }

    let mut lines = Vec::new();
    let mut all_matched = true;
    let mut any_over = false;
    for item in &items {
        let ordered = item.quantity_ordered;
        let received = item.quantity_received.unwrap_or_default();
        let invoiced = item.quantity_invoiced.unwrap_or_default();
        let over_received = received > ordered;
        let over_invoiced = invoiced > ordered;
        let match_status = if over_received || over_invoiced {
            all_matched = false;
            any_over = true;
            "over"
        } else if received == ordered && invoiced == ordered {
            "matched"
        } else if received.is_zero() && invoiced.is_zero() {
            all_matched = false;
            "unmatched"
        } else {
            all_matched = false;
            "partial"
        };
        lines.push(json!({
            "po_item_id": item.id.to_string(),
            "component_sku": item.component_sku,
            "description": item.description,
            "quantity_ordered": dec_float(&ordered),
            "quantity_received": dec_float(&received),
            "quantity_invoiced": dec_float(&invoiced),
            "unit_price": dec_float(&item.unit_price),
            "currency": item.currency,
            "match_status": match_status,
            "over_received": over_received,
            "over_invoiced": over_invoiced,
            "receipt_pct": pct(received, ordered),
            "invoice_pct": pct(invoiced, ordered),
        }));
    }
    let mut total_invoiced = Map::new();
    for (cur, amt) in invoiced_by_currency {
        total_invoiced.insert(cur, dec_float(&amt));
    }
    Ok(resp::json(json!({
        "po_id": po_id,
        "po_number": po.number,
        "po_status": po.status,
        "po_total": dec_float(&po.total.unwrap_or_default()),
        "po_currency": po.currency,
        // {currency: amount}: invoices in different currencies are never added together.
        "total_invoiced": Value::Object(total_invoiced),
        "overall_match": if any_over {
            "over"
        } else if all_matched {
            "matched"
        } else {
            "partial"
        },
        "invoice_count": invoices.len(),
        "receipt_count": receipt_count,
        "lines": lines,
    })))
}
