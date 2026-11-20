//! backend/app/routers/inventory.py, mounted at `/api/v1/inventory` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Ids are `str` compared with UUID columns: a malformed one is asyncpg's error when the statement runs
//! (the central 422), see [`pg_uuid`].
//!
//! Python quirks kept:
//! - the location tree walks `StockLocation.children`, selectin-loaded for the warehouse's locations
//!   only: a child that lives in another warehouse would need a lazy load, which an AsyncSession refuses
//!   (500);
//! - PATCH /warehouses/{id} and /locations/{id} take the full create models (name and code required).

use crate::extract::{pg_uuid, pg_uuid_opt};
use std::collections::HashMap;

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::D;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};

use super::assets::now_micros;
use super::at;
use crate::resp::{self, dec_float, dec_str, iso, py_float};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/inventory";

/// The routes of `router` in inventory.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/warehouses"),
            get(list_warehouses).post(create_warehouse),
        )
        .route(
            &at(PREFIX, "/warehouses/{warehouse_id}"),
            patch(update_warehouse).delete(deactivate_warehouse),
        )
        .route(
            &at(PREFIX, "/warehouses/{warehouse_id}/locations"),
            get(get_warehouse_locations),
        )
        .route(
            &at(PREFIX, "/locations"),
            get(list_all_locations).post(create_location),
        )
        .route(
            &at(PREFIX, "/locations/{location_id}"),
            patch(update_location).delete(delete_location),
        )
        .route(
            &at(PREFIX, "/stock/moves"),
            get(list_moves).post(record_move),
        )
        .route(&at(PREFIX, "/stock/transfer"), post(transfer_stock))
        .route(&at(PREFIX, "/stock/ledger/{sku}"), get(stock_ledger))
}

// ─── Permissions ─────────────────────────────────────────────────────────────

fn require_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !user.perms.contains("inventory:view") && !user.perms.contains("inventory:manage") {
        return Err(ApiError::http(403, "Permission required: inventory:view"));
    }
    Ok(())
}

fn require_manage(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !user.perms.contains("inventory:manage") {
        return Err(ApiError::http(403, "Permission required: inventory:manage"));
    }
    Ok(())
}

fn truthy(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|x| !x.is_empty())
}

// ─── Locations and the tree ──────────────────────────────────────────────────

#[derive(sqlx::FromRow, Clone, Debug)]
struct LocationRow {
    id: Uuid,
    warehouse_id: Uuid,
    parent_id: Option<Uuid>,
    name: String,
    code: String,
    location_type: Option<String>,
    is_active: bool,
    created_at: DateTime<Utc>,
}

const LOCATION_COLS: &str = "stock_locations.id, stock_locations.warehouse_id, stock_locations.parent_id, \
     stock_locations.name, stock_locations.code, stock_locations.location_type, stock_locations.is_active, \
     stock_locations.created_at";

/// `_build_path`: walk up the parent chain (within `loc_map`) to "A / B / C".
fn build_path(loc: &LocationRow, loc_map: &HashMap<Uuid, &LocationRow>) -> String {
    let mut parts = vec![loc.name.clone()];
    let mut current = loc;
    let mut seen: Vec<Uuid> = Vec::new();
    while let Some(pid) = current.parent_id {
        if seen.contains(&pid) {
            break;
        }
        seen.push(current.id);
        let Some(parent) = loc_map.get(&pid) else {
            break;
        };
        parts.insert(0, parent.name.clone());
        current = parent;
    }
    parts.join(" / ")
}

fn location_dict(loc: &LocationRow, full_path: &str) -> Map<String, Value> {
    let v = json!({
        "id": loc.id.to_string(),
        "warehouse_id": loc.warehouse_id.to_string(),
        "parent_id": loc.parent_id.map(|p| p.to_string()),
        "name": loc.name,
        "code": loc.code,
        "location_type": loc.location_type,
        "full_path": full_path,
        "is_active": loc.is_active,
        "created_at": iso(&loc.created_at),
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

/// `sorted(xs, key=lambda x: x.name)` (stable, code-point order).
fn by_name(xs: &mut [LocationRow]) {
    xs.sort_by(|a, b| a.name.cmp(&b.name));
}

/// `_location_tree(locs)`: the nested tree, built from the flat list only. A location whose parent is not
/// in the list (none, or in another warehouse) is a root; `seen` stops a parent cycle.
fn location_tree(locs: &[LocationRow]) -> Vec<Value> {
    let loc_map: HashMap<Uuid, &LocationRow> = locs.iter().map(|l| (l.id, l)).collect();
    let mut sorted: Vec<LocationRow> = locs.to_vec();
    by_name(&mut sorted);
    let mut children: HashMap<Uuid, Vec<LocationRow>> = HashMap::new();
    let mut roots: Vec<LocationRow> = Vec::new();
    for loc in sorted {
        match loc.parent_id {
            Some(p) if loc_map.contains_key(&p) && p != loc.id => {
                children.entry(p).or_default().push(loc)
            }
            _ => roots.push(loc),
        }
    }
    fn node(
        loc: &LocationRow,
        seen: &[Uuid],
        loc_map: &HashMap<Uuid, &LocationRow>,
        children: &HashMap<Uuid, Vec<LocationRow>>,
    ) -> Value {
        let full_path = build_path(loc, loc_map);
        let mut d = location_dict(loc, &full_path);
        let mut seen = seen.to_vec();
        seen.push(loc.id);
        let kids: Vec<Value> = children
            .get(&loc.id)
            .map(|cs| {
                cs.iter()
                    .filter(|c| !seen.contains(&c.id))
                    .map(|c| node(c, &seen, loc_map, children))
                    .collect()
            })
            .unwrap_or_default();
        d.insert("children".into(), Value::Array(kids));
        Value::Object(d)
    }
    roots
        .iter()
        .map(|r| node(r, &[], &loc_map, &children))
        .collect()
}

// ─── Warehouse endpoints ──────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class WarehouseCreate(BaseModel)`.
    pub struct WarehouseCreate {
        pub name: String,
        pub code: String,
        pub address: Option<String> = None,
        pub is_default: bool = false,
    }
}

#[derive(sqlx::FromRow, Clone, Debug)]
struct WarehouseRow {
    id: Uuid,
    name: String,
    code: String,
    address: Option<String>,
    is_default: bool,
    is_active: bool,
    created_at: DateTime<Utc>,
}

const WAREHOUSE_COLS: &str =
    "warehouses.id, warehouses.name, warehouses.code, warehouses.address, \
     warehouses.is_default, warehouses.is_active, warehouses.created_at";

/// `GET /warehouses`: the active ones.
pub async fn list_warehouses(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query_as::<_, WarehouseRow>(&format!(
        "SELECT {WAREHOUSE_COLS} FROM warehouses WHERE warehouses.is_active = true ORDER BY warehouses.name"
    ))
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|w| {
            json!({
                "id": w.id.to_string(),
                "name": w.name,
                "code": w.code,
                "address": w.address,
                "is_default": w.is_default,
                "is_active": w.is_active,
                "created_at": iso(&w.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// Unset `is_default` on every warehouse that has it (one UPDATE per row, as the ORM flushes them).
async fn clear_defaults(db: &mut PgConnection) -> ApiResult<()> {
    let ids: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM warehouses WHERE is_default = true")
        .fetch_all(&mut *db)
        .await?;
    for (id,) in ids {
        sqlx::query("UPDATE warehouses SET is_default = false, updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}

/// `POST /warehouses` (201).
pub async fn create_warehouse(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<WarehouseCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    if body.is_default {
        clear_defaults(&mut conn).await?;
    }
    let wh = sqlx::query_as::<_, WarehouseRow>(&format!(
        "INSERT INTO warehouses (name, code, address, is_default, is_active, id) \
         VALUES ($1, $2, $3, $4, true, $5) RETURNING {}",
        WAREHOUSE_COLS.replace("warehouses.", "")
    ))
    .bind(&body.name)
    .bind(body.code.to_uppercase())
    .bind(&body.address)
    .bind(body.is_default)
    .bind(Uuid::new_v4())
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "id": wh.id.to_string(), "name": wh.name, "code": wh.code,
            "address": wh.address, "is_default": wh.is_default, "is_active": wh.is_active,
        }),
    ))
}

async fn find_warehouse(db: &mut PgConnection, warehouse_id: &str) -> ApiResult<WarehouseRow> {
    let id = pg_uuid(warehouse_id)?;
    sqlx::query_as::<_, WarehouseRow>(&format!(
        "SELECT {WAREHOUSE_COLS} FROM warehouses WHERE warehouses.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?
    .ok_or_else(|| ApiError::http(404, "Warehouse not found"))
}

/// `PATCH /warehouses/{warehouse_id}`: the full create model (`exclude_none`), code upper-cased.
pub async fn update_warehouse(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(warehouse_id): Path<String>,
    PyJson(body): PyJson<WarehouseCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let wh = find_warehouse(&mut conn, &warehouse_id).await?;
    if body.is_default && !wh.is_default {
        clear_defaults(&mut conn).await?;
    }
    let code = body.code.to_uppercase();
    let address = body.address.clone().or(wh.address.clone());
    if body.name != wh.name
        || code != wh.code
        || address != wh.address
        || body.is_default != wh.is_default
    {
        sqlx::query(
            "UPDATE warehouses SET name = $1, code = $2, address = $3, is_default = $4, updated_at = now() \
             WHERE id = $5",
        )
        .bind(&body.name)
        .bind(&code)
        .bind(&address)
        .bind(body.is_default)
        .bind(wh.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let wh = find_warehouse(&mut conn, &wh.id.to_string()).await?;
    Ok(resp::json(json!({
        "id": wh.id.to_string(), "name": wh.name, "code": wh.code, "is_default": wh.is_default,
    })))
}

/// `DELETE /warehouses/{warehouse_id}` (204): deactivates (never the default).
pub async fn deactivate_warehouse(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(warehouse_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let wh = find_warehouse(&mut conn, &warehouse_id).await?;
    if wh.is_default {
        return Err(ApiError::http(
            400,
            "Cannot deactivate the default warehouse",
        ));
    }
    if wh.is_active {
        sqlx::query("UPDATE warehouses SET is_active = false, updated_at = now() WHERE id = $1")
            .bind(wh.id)
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Location endpoints ───────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class LocationCreate(BaseModel)`.
    pub struct LocationCreate {
        pub warehouse_id: String,
        pub parent_id: Option<String> = None,
        pub name: String,
        pub code: String,
        pub location_type: String = "bin",
    }
}

/// `GET /warehouses/{warehouse_id}/locations`: the warehouse's locations as a nested tree.
pub async fn get_warehouse_locations(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(warehouse_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let wh = find_warehouse(&mut conn, &warehouse_id).await?;
    let locs = sqlx::query_as::<_, LocationRow>(&format!(
        "SELECT {LOCATION_COLS} FROM stock_locations WHERE stock_locations.warehouse_id = $1 \
         ORDER BY stock_locations.name"
    ))
    .bind(wh.id)
    .fetch_all(&mut *conn)
    .await?;
    let out = location_tree(&locs);
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// The query parameters of `list_all_locations`.
    pub struct ListLocationsQ {
        pub warehouse_id: Option<String> = None,
    }
}

/// `GET /locations`: a flat list of the active locations (for dropdowns).
pub async fn list_all_locations(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListLocationsQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let locs = match truthy(&q.warehouse_id) {
        Some(w) => {
            let wid = pg_uuid(w)?;
            sqlx::query_as::<_, LocationRow>(&format!(
                "SELECT {LOCATION_COLS} FROM stock_locations WHERE stock_locations.is_active = true \
                 AND stock_locations.warehouse_id = $1 ORDER BY stock_locations.name"
            ))
            .bind(wid)
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query_as::<_, LocationRow>(&format!(
            "SELECT {LOCATION_COLS} FROM stock_locations WHERE stock_locations.is_active = true \
                 ORDER BY stock_locations.name"
        ))
            .fetch_all(&mut *conn)
            .await?
        }
    };
    let loc_map: HashMap<Uuid, &LocationRow> = locs.iter().map(|l| (l.id, l)).collect();
    let out: Vec<Value> = locs
        .iter()
        .map(|l| Value::Object(location_dict(l, &build_path(l, &loc_map))))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /locations` (201).
pub async fn create_location(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<LocationCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let warehouse_id = pg_uuid(&body.warehouse_id)?;
    let parent_id = pg_uuid_opt(truthy(&body.parent_id))?;
    let loc = sqlx::query_as::<_, LocationRow>(&format!(
        "INSERT INTO stock_locations (warehouse_id, parent_id, name, code, location_type, is_active, id) \
         VALUES ($1, $2, $3, $4, $5, true, $6) RETURNING {}",
        LOCATION_COLS.replace("stock_locations.", "")
    ))
    .bind(warehouse_id)
    .bind(parent_id)
    .bind(&body.name)
    .bind(body.code.to_uppercase())
    .bind(&body.location_type)
    .bind(Uuid::new_v4())
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        Value::Object(location_dict(&loc, &loc.name)),
    ))
}

async fn find_location(db: &mut PgConnection, location_id: &str) -> ApiResult<LocationRow> {
    let id = pg_uuid(location_id)?;
    sqlx::query_as::<_, LocationRow>(&format!(
        "SELECT {LOCATION_COLS} FROM stock_locations WHERE stock_locations.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?
    .ok_or_else(|| ApiError::http(404, "Location not found"))
}

/// `PATCH /locations/{location_id}`: name, code, type, and the parent when one is given.
pub async fn update_location(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(location_id): Path<String>,
    PyJson(body): PyJson<LocationCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let loc = find_location(&mut conn, &location_id).await?;
    let code = body.code.to_uppercase();
    let parent_id = match truthy(&body.parent_id) {
        Some(p) => Some(pg_uuid(p)?),
        None => loc.parent_id,
    };
    if body.name != loc.name
        || code != loc.code
        || Some(&body.location_type) != loc.location_type.as_ref()
        || parent_id != loc.parent_id
    {
        sqlx::query(
            "UPDATE stock_locations SET name = $1, code = $2, location_type = $3, parent_id = $4, \
             updated_at = now() WHERE id = $5",
        )
        .bind(&body.name)
        .bind(&code)
        .bind(&body.location_type)
        .bind(parent_id)
        .bind(loc.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let loc = find_location(&mut conn, &loc.id.to_string()).await?;
    Ok(resp::json(Value::Object(location_dict(&loc, &loc.name))))
}

/// `DELETE /locations/{location_id}` (204): deactivates.
pub async fn delete_location(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(location_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let loc = find_location(&mut conn, &location_id).await?;
    if loc.is_active {
        sqlx::query(
            "UPDATE stock_locations SET is_active = false, updated_at = now() WHERE id = $1",
        )
        .bind(loc.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Stock moves ─────────────────────────────────────────────────────────────

/// `MOVE_TYPES`.
const MOVE_TYPES: [&str; 4] = ["receipt", "issue", "internal", "adjustment"];

granary_core::py_model! {
    /// `class StockMoveCreate(BaseModel)`.
    pub struct StockMoveCreate {
        pub component_sku: String,
        pub component_name: Option<String> = None,
        pub from_location_id: Option<String> = None,
        pub to_location_id: Option<String> = None,
        pub quantity: f64,
        /// receipt | issue | internal | adjustment
        pub move_type: String,
        pub reference: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

#[derive(sqlx::FromRow, Clone, Debug)]
struct MoveRow {
    id: Uuid,
    component_sku: String,
    component_name: Option<String>,
    from_location_id: Option<Uuid>,
    from_location_name: Option<String>,
    to_location_id: Option<Uuid>,
    to_location_name: Option<String>,
    quantity: Decimal,
    move_type: String,
    reference: Option<String>,
    notes: Option<String>,
    moved_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

const MOVE_SELECT: &str = "SELECT stock_moves.id, stock_moves.component_sku, stock_moves.component_name, \
     stock_moves.from_location_id, fl.name AS from_location_name, stock_moves.to_location_id, \
     tl.name AS to_location_name, stock_moves.quantity, stock_moves.move_type, stock_moves.reference, \
     stock_moves.notes, stock_moves.moved_at, stock_moves.created_at FROM stock_moves \
     LEFT OUTER JOIN stock_locations AS fl ON fl.id = stock_moves.from_location_id \
     LEFT OUTER JOIN stock_locations AS tl ON tl.id = stock_moves.to_location_id";

fn move_dict(m: &MoveRow) -> Value {
    json!({
        "id": m.id.to_string(),
        "component_sku": m.component_sku,
        "component_name": m.component_name,
        "from_location_id": m.from_location_id.map(|x| x.to_string()),
        "from_location_name": m.from_location_name,
        "to_location_id": m.to_location_id.map(|x| x.to_string()),
        "to_location_name": m.to_location_name,
        "quantity": dec_float(&m.quantity),
        "move_type": m.move_type,
        "reference": m.reference,
        "notes": m.notes,
        "moved_at": iso(&m.moved_at),
        "created_at": iso(&m.created_at),
    })
}

granary_core::py_model! {
    /// The query parameters of `list_moves`.
    pub struct ListMovesQ {
        pub component_sku: Option<String> = None,
        pub location_id: Option<String> = None,
        pub move_type: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `GET /stock/moves`.
pub async fn list_moves(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListMovesQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let sku = truthy(&q.component_sku).map(|s| format!("%{s}%"));
    let mt = truthy(&q.move_type).map(str::to_string);
    let loc = match truthy(&q.location_id) {
        Some(l) => Some(pg_uuid(l)?),
        None => None,
    };
    let mut sql = format!("{MOVE_SELECT} WHERE true");
    let mut n = 0;
    if sku.is_some() {
        n += 1;
        sql += &format!(" AND stock_moves.component_sku ILIKE ${n}");
    }
    if mt.is_some() {
        n += 1;
        sql += &format!(" AND stock_moves.move_type = ${n}");
    }
    if loc.is_some() {
        n += 1;
        sql += &format!(
            " AND (stock_moves.from_location_id = ${n} OR stock_moves.to_location_id = ${n})"
        );
    }
    sql += &format!(
        " ORDER BY stock_moves.moved_at DESC LIMIT ${}::integer OFFSET ${}::integer",
        n + 1,
        n + 2
    );
    let mut query = sqlx::query_as::<_, MoveRow>(&sql);
    if let Some(s) = &sku {
        query = query.bind(s);
    }
    if let Some(m) = &mt {
        query = query.bind(m);
    }
    if let Some(l) = loc {
        query = query.bind(l);
    }
    let rows = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(move_dict).collect(),
    )))
}

#[derive(sqlx::FromRow, Clone, Debug)]
struct InvRow {
    id: Uuid,
    component_name: String,
    quantity_on_hand: Option<Decimal>,
    quantity_reserved: Option<Decimal>,
    quantity_available: Option<Decimal>,
    location_id: Option<Uuid>,
    updated_by_id: Option<Uuid>,
}

const INV_SELECT: &str = "SELECT component_inventory.id, component_inventory.component_name, \
     component_inventory.quantity_on_hand, component_inventory.quantity_reserved, \
     component_inventory.quantity_available, component_inventory.location_id, \
     component_inventory.updated_by_id FROM component_inventory";

async fn move_by_id(db: &mut PgConnection, id: Uuid) -> ApiResult<MoveRow> {
    Ok(
        sqlx::query_as::<_, MoveRow>(&format!("{MOVE_SELECT} WHERE stock_moves.id = $1"))
            .bind(id)
            .fetch_one(&mut *db)
            .await?,
    )
}

/// `POST /stock/moves` (201): one movement, and the component's quantity on hand.
pub async fn record_move(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<StockMoveCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    if !MOVE_TYPES.contains(&body.move_type.as_str()) {
        return Err(ApiError::http(
            400,
            format!("move_type must be one of: {}", MOVE_TYPES.join(", ")),
        ));
    }
    let qty = D(body.quantity);
    // An adjustment is a signed delta; the others are amounts.
    if body.move_type == "adjustment" && qty.is_zero() {
        return Err(ApiError::http(400, "An adjustment quantity can't be zero"));
    }
    if body.move_type != "adjustment" && qty <= Decimal::ZERO {
        return Err(ApiError::http(400, "Quantity must be greater than zero"));
    }
    let mut conn = db.conn().await?;
    let inv = sqlx::query_as::<_, InvRow>(&format!(
        "{INV_SELECT} WHERE component_inventory.component_sku = $1 FOR UPDATE"
    ))
    .bind(&body.component_sku)
    .fetch_optional(&mut *conn)
    .await?;
    let to_location = truthy(&body.to_location_id);
    let Some(inv) = inv else {
        return Err(ApiError::http(
            404,
            format!("{} is not in inventory", body.component_sku),
        ));
    };
    {
        let mut on_hand = D(inv.quantity_on_hand);
        let mut location: Option<&str> = None;
        match body.move_type.as_str() {
            "receipt" | "adjustment" => {
                on_hand += qty;
                location = to_location;
            }
            "issue" => on_hand -= qty,
            _ => location = to_location, // internal: quantity unchanged, only the location
        }
        if on_hand < Decimal::ZERO {
            return Err(ApiError::http(
                400,
                format!(
                    "Only {} of {} on hand; this move would make it negative",
                    dec_str(&D(inv.quantity_on_hand)),
                    body.component_sku
                ),
            ));
        }
        let available = (on_hand - D(inv.quantity_reserved)).max(Decimal::ZERO);
        let new_loc = match location {
            Some(l) => Some(pg_uuid(l)?),
            None => inv.location_id,
        };
        if Some(on_hand) != inv.quantity_on_hand
            || Some(available) != inv.quantity_available
            || Some(user.id) != inv.updated_by_id
            || new_loc != inv.location_id
        {
            sqlx::query(
                "UPDATE component_inventory SET quantity_on_hand = $1, quantity_available = $2, \
                 updated_by_id = $3, location_id = $4, updated_at = now() WHERE id = $5",
            )
            .bind(on_hand)
            .bind(available)
            .bind(user.id)
            .bind(new_loc)
            .bind(inv.id)
            .execute(&mut *conn)
            .await?;
        }
    }
    let from_id = pg_uuid_opt(truthy(&body.from_location_id))?;
    let to_id = pg_uuid_opt(to_location)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO stock_moves (component_sku, component_name, from_location_id, to_location_id, quantity, \
         move_type, reference, moved_by_id, moved_at, notes, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, \
         $10, $11)",
    )
    .bind(&body.component_sku)
    .bind(&body.component_name)
    .bind(from_id)
    .bind(to_id)
    .bind(qty)
    .bind(&body.move_type)
    .bind(&body.reference)
    .bind(user.id)
    .bind(now_micros())
    .bind(&body.notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let m = move_by_id(&mut conn, id).await?;
    Ok(resp::json_status(201, move_dict(&m)))
}

/// `POST /stock/transfer` (201): an issue from one location and a receipt at the other.
pub async fn transfer_stock(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<StockMoveCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let (Some(from), Some(to)) = (truthy(&body.from_location_id), truthy(&body.to_location_id))
    else {
        return Err(ApiError::http(
            400,
            "Both from_location_id and to_location_id are required for a transfer",
        ));
    };
    let qty = D(body.quantity);
    if qty <= Decimal::ZERO {
        return Err(ApiError::http(400, "Quantity must be greater than zero"));
    }
    let mut conn = db.conn().await?;
    let inv = sqlx::query_as::<_, InvRow>(&format!(
        "{INV_SELECT} WHERE component_inventory.component_sku = $1 FOR UPDATE"
    ))
    .bind(&body.component_sku)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, format!("{} is not in inventory", body.component_sku)))?;
    if qty > D(inv.quantity_on_hand) {
        return Err(ApiError::http(
            400,
            format!(
                "Only {} of {} on hand to transfer",
                dec_str(&D(inv.quantity_on_hand)),
                body.component_sku
            ),
        ));
    }
    let now = now_micros();
    let from_id = pg_uuid(from)?;
    let to_id = pg_uuid(to)?;
    for (fid, tid, mt, notes) in [
        (
            Some(from_id),
            None,
            "issue",
            format!("Transfer out \u{2192} {to}"),
        ),
        (
            None,
            Some(to_id),
            "receipt",
            format!("Transfer in \u{2190} {from}"),
        ),
    ] {
        sqlx::query(
            "INSERT INTO stock_moves (component_sku, component_name, from_location_id, to_location_id, \
             quantity, move_type, reference, moved_by_id, moved_at, notes, id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&body.component_sku)
        .bind(&body.component_name)
        .bind(fid)
        .bind(tid)
        .bind(qty)
        .bind(mt)
        .bind(&body.reference)
        .bind(user.id)
        .bind(now)
        .bind(&notes)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    if inv.location_id != Some(to_id) {
        sqlx::query(
            "UPDATE component_inventory SET location_id = $1, updated_at = now() WHERE id = $2",
        )
        .bind(to_id)
        .bind(inv.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"status": "transferred", "sku": body.component_sku, "quantity": py_float(body.quantity)}),
    ))
}

granary_core::py_model! {
    /// The query parameters of `stock_ledger`.
    pub struct LedgerQ {
        pub limit: i64 = 200,
    }
}

/// `GET /stock/ledger/{sku}`: the movement history of one component.
pub async fn stock_ledger(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(sku): Path<String>,
    PyQuery(q): PyQuery<LedgerQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let moves = sqlx::query_as::<_, MoveRow>(&format!(
        "{MOVE_SELECT} WHERE stock_moves.component_sku = $1 ORDER BY stock_moves.moved_at DESC \
         LIMIT $2::integer"
    ))
    .bind(&sku)
    .bind(q.limit)
    .fetch_all(&mut *conn)
    .await?;
    let inv = sqlx::query_as::<_, InvRow>(&format!(
        "{INV_SELECT} WHERE component_inventory.component_sku = $1"
    ))
    .bind(&sku)
    .fetch_optional(&mut *conn)
    .await?;
    let on_hand: Value = match &inv {
        None => json!(0),
        Some(i) => match &i.quantity_on_hand {
            Some(qoh) => dec_float(qoh),
            // float(None): TypeError
            None => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "float() argument must be a string or a real number, not 'NoneType'"
                )))
            }
        },
    };
    Ok(resp::json(json!({
        "sku": sku,
        "component_name": inv.as_ref().map(|i| i.component_name.clone()),
        "current_quantity_on_hand": on_hand,
        "current_location_id": inv.as_ref().and_then(|i| i.location_id).map(|l| l.to_string()),
        "moves": moves.iter().map(move_dict).collect::<Vec<_>>(),
    })))
}
