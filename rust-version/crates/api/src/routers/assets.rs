//! backend/app/routers/assets.py, mounted at `/api/v1/finance` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Shares `/api/v1/finance` with finance.py (group 8) and budget.py: no path of this file collides with
//! theirs (`/categories`, `/assets/...`).
//!
//! Also home of two small helpers the group-9 routers share ([`asyncpg_uuid`], [`in_marks`]).
//!
//! Python quirks kept:
//! - path and body ids are plain `str` compared with UUID columns: asyncpg refuses a malformed one when
//!   the statement runs, an unhandled error (500), at the point Python runs that statement;
//! - PATCH /categories/{id} takes the full `CategoryCreate` model with `exclude_none`, so the defaults
//!   (`depreciation_method="straight_line"`, `salvage_value_pct=0`) overwrite the row when not sent.

use crate::extract::{pg_uuid, pg_uuid_opt};
use std::collections::HashMap;

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_core::validation::v;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::fx::{functional_currency, require_rate};
use granary_services::gl_posting::{self, AssetCategoryDoc, AssetDoc, DisposalDoc};
use granary_services::numbering::next_document_number;

use super::at;
use crate::resp::{self, date_iso, dec_float, iso};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/finance";

/// The routes of `router` in assets.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/categories"),
            get(list_categories).post(create_category),
        )
        .route(
            &at(PREFIX, "/categories/{category_id}"),
            patch(update_category).delete(delete_category),
        )
        .route(&at(PREFIX, "/assets"), get(list_assets).post(create_asset))
        .route(
            &at(PREFIX, "/assets/{asset_id}"),
            get(get_asset).patch(update_asset).delete(delete_asset),
        )
        .route(
            &at(PREFIX, "/assets/{asset_id}/depreciation-schedule"),
            get(get_depreciation_schedule),
        )
        .route(
            &at(PREFIX, "/assets/{asset_id}/generate-schedule"),
            post(generate_schedule),
        )
        .route(
            &at(PREFIX, "/assets/{asset_id}/post-depreciation"),
            post(post_depreciation),
        )
        .route(
            &at(PREFIX, "/assets/{asset_id}/dispose"),
            post(dispose_asset),
        )
}

// ─── Shared helpers (group 9) ────────────────────────────────────────────────

/// `$start, $start+1, ...` for an expanded `IN (...)` list of `n` values (SQLAlchemy's selectin shape).
pub fn in_marks(start: usize, n: usize) -> String {
    (start..start + n)
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `float(x) if x else 0`: the int 0 for zero or NULL.
pub fn float_or_zero(d: Option<Decimal>) -> Value {
    match d {
        Some(x) if !x.is_zero() => dec_float(&x),
        _ => json!(0),
    }
}

// ─── Permissions ─────────────────────────────────────────────────────────────

fn require_asset_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !user.perms.contains("finance:view_assets") && !user.perms.contains("finance:manage_assets")
    {
        return Err(ApiError::http(
            403,
            "Permission required: finance:view_assets",
        ));
    }
    Ok(())
}

fn require_asset_manage(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !user.perms.contains("finance:manage_assets") {
        return Err(ApiError::http(
            403,
            "Permission required: finance:manage_assets",
        ));
    }
    Ok(())
}

// ─── Rows ────────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct CategoryRow {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub depreciation_method: Option<String>,
    pub useful_life_months: Option<i32>,
    pub salvage_value_pct: Option<Decimal>,
    pub gl_asset_account_id: Option<Uuid>,
    pub gl_depreciation_account_id: Option<Uuid>,
    pub gl_accumulated_account_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

const CATEGORY_COLS: &str =
    "asset_categories.id, asset_categories.name, asset_categories.description, \
     asset_categories.depreciation_method, asset_categories.useful_life_months, \
     asset_categories.salvage_value_pct, asset_categories.gl_asset_account_id, \
     asset_categories.gl_depreciation_account_id, asset_categories.gl_accumulated_account_id, \
     asset_categories.created_at";

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct AssetRow {
    pub id: Uuid,
    pub number: String,
    pub name: String,
    pub description: Option<String>,
    pub category_id: Option<Uuid>,
    pub status: Option<String>,
    pub purchase_date: NaiveDate,
    pub purchase_price: Decimal,
    pub currency: Option<String>,
    pub acquisition_rate: Option<Decimal>,
    pub useful_life_months: Option<i32>,
    pub salvage_value: Option<Decimal>,
    pub depreciation_method: Option<String>,
    pub location: Option<String>,
    pub serial_number: Option<String>,
    pub vendor_id: Option<Uuid>,
    pub purchase_order_id: Option<Uuid>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

const ASSET_COLS: &str = "fixed_assets.id, fixed_assets.number, fixed_assets.name, fixed_assets.description, \
     fixed_assets.category_id, fixed_assets.status, fixed_assets.purchase_date, fixed_assets.purchase_price, \
     fixed_assets.currency, fixed_assets.acquisition_rate, fixed_assets.useful_life_months, \
     fixed_assets.salvage_value, fixed_assets.depreciation_method, fixed_assets.location, \
     fixed_assets.serial_number, fixed_assets.vendor_id, fixed_assets.purchase_order_id, fixed_assets.notes, \
     fixed_assets.created_at";

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct LineRow {
    pub id: Uuid,
    pub asset_id: Uuid,
    pub period_date: NaiveDate,
    pub period_label: Option<String>,
    pub depreciation_amount: Decimal,
    pub accumulated_depreciation: Decimal,
    pub book_value: Decimal,
    pub journal_entry_id: Option<Uuid>,
    pub status: Option<String>,
}

const LINE_COLS: &str = "asset_depreciation_lines.id, asset_depreciation_lines.asset_id, \
     asset_depreciation_lines.period_date, asset_depreciation_lines.period_label, \
     asset_depreciation_lines.depreciation_amount, asset_depreciation_lines.accumulated_depreciation, \
     asset_depreciation_lines.book_value, asset_depreciation_lines.journal_entry_id, \
     asset_depreciation_lines.status";

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct DisposalRow {
    pub id: Uuid,
    pub asset_id: Uuid,
    pub disposal_date: NaiveDate,
    pub disposal_type: String,
    pub proceeds: Option<Decimal>,
    pub disposal_notes: Option<String>,
    pub journal_entry_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

const DISPOSAL_COLS: &str =
    "asset_disposals.id, asset_disposals.asset_id, asset_disposals.disposal_date, \
     asset_disposals.disposal_type, asset_disposals.proceeds, asset_disposals.disposal_notes, \
     asset_disposals.journal_entry_id, asset_disposals.created_at";

/// An asset with its `category` (selectinload; soft-deleted categories load too).
struct Loaded {
    asset: AssetRow,
    category: Option<CategoryRow>,
}

async fn load_category(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<CategoryRow>> {
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query_as::<_, CategoryRow>(&format!(
        "SELECT {CATEGORY_COLS} FROM asset_categories WHERE asset_categories.id IN ($1)"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

/// `select(FixedAsset).options(selectinload(category)).where(id == asset_id, is_deleted == False)`.
async fn load_asset(
    db: &mut PgConnection,
    asset_id: &str,
    for_update: bool,
) -> ApiResult<Option<Loaded>> {
    let id = pg_uuid(asset_id)?;
    let asset = sqlx::query_as::<_, AssetRow>(&format!(
        "SELECT {ASSET_COLS} FROM fixed_assets WHERE fixed_assets.id = $1 AND fixed_assets.is_deleted = false{}",
        if for_update { " FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(asset) = asset else { return Ok(None) };
    let category = load_category(db, asset.category_id).await?;
    Ok(Some(Loaded { asset, category }))
}

/// `FixedAsset.depreciation_lines` (order_by period_date).
async fn load_lines(db: &mut PgConnection, asset_id: Uuid) -> ApiResult<Vec<LineRow>> {
    Ok(sqlx::query_as::<_, LineRow>(&format!(
        "SELECT {LINE_COLS} FROM asset_depreciation_lines WHERE asset_depreciation_lines.asset_id IN ($1) \
         ORDER BY asset_depreciation_lines.period_date"
    ))
    .bind(asset_id)
    .fetch_all(&mut *db)
    .await?)
}

// ─── Schedule ────────────────────────────────────────────────────────────────

/// What `_calculate_schedule` reads from the asset.
#[derive(Clone, Debug)]
pub struct ScheduleAsset {
    pub depreciation_method: Option<String>,
    pub useful_life_months: Option<i32>,
    pub purchase_price: Decimal,
    pub salvage_value: Option<Decimal>,
    pub purchase_date: NaiveDate,
}

/// What `_calculate_schedule` reads from the category.
#[derive(Clone, Debug)]
pub struct ScheduleCategory {
    pub depreciation_method: Option<String>,
    pub useful_life_months: Option<i32>,
}

/// One period of `_calculate_schedule` (amounts as Decimals).
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduleLine {
    pub period_number: i64,
    pub period_date: NaiveDate,
    pub period_label: String,
    pub depreciation_amount: Decimal,
    pub accumulated_depreciation: Decimal,
    pub book_value: Decimal,
}

fn truthy(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|x| !x.is_empty())
}

/// `_calculate_schedule(asset, category, places)`: pure. Amounts are rounded to `places`, the last
/// period takes the rounding residue so the schedule ends at salvage. Double-declining balance switches
/// to straight-line as soon as that charges at least as much.
///
/// Divisions and the declining-balance product follow Python's default decimal context (28 significant
/// digits, ROUND_HALF_EVEN) through [`pyd`], so a value on a half-unit boundary rounds as Python's does.
pub fn calculate_schedule(
    asset: &ScheduleAsset,
    category: Option<&ScheduleCategory>,
    places: u32,
) -> Vec<ScheduleLine> {
    // asset.depreciation_method or (category.depreciation_method if category else "straight_line")
    let method: Option<String> = match truthy(&asset.depreciation_method) {
        Some(m) => Some(m.to_string()),
        None => match category {
            Some(c) => c.depreciation_method.clone(),
            None => Some("straight_line".into()),
        },
    };
    let useful_life: Option<i32> = match asset.useful_life_months.filter(|n| *n != 0) {
        Some(n) => Some(n),
        None => match category {
            Some(c) => c.useful_life_months,
            None => Some(0),
        },
    };
    let useful_life = match useful_life {
        Some(n) if n > 0 => n,
        _ => return Vec::new(),
    };
    if method.as_deref() == Some("none") {
        return Vec::new();
    }
    let salvage = quantize_money(D(asset.salvage_value), places);
    let price = quantize_money(asset.purchase_price, places);
    let depreciable = price - salvage;
    if depreciable <= Decimal::ZERO {
        return Vec::new();
    }
    let life = Decimal::from(useful_life);
    let mut lines = Vec::new();
    let mut book_value = price;
    let mut accumulated = Decimal::ZERO;
    let straight = method.as_deref() == Some("straight_line");
    let mut switched = straight;
    let sl_amount = pyd::quantize_half_up(&pyd::div28(depreciable, life), places);
    let ddb_rate = pyd::div28(Decimal::TWO, life);
    let (y0, m0) = (
        asset.purchase_date.year(),
        asset.purchase_date.month0() as i32,
    );
    for i in 0..useful_life {
        let m = m0 + i;
        let period_date =
            NaiveDate::from_ymd_opt(y0 + m.div_euclid(12), (m.rem_euclid(12) + 1) as u32, 1)
                .expect("first of month");
        let period_label = granary_compat::pyfmt::strftime_date(period_date, "%Y-%m");
        let remaining = useful_life - i;
        let left = book_value - salvage;
        let mut dep_amount = if remaining == 1 {
            left
        } else if straight {
            sl_amount
        } else {
            let sl_now = pyd::quantize_half_up(&pyd::div28(left, Decimal::from(remaining)), places);
            let ddb = pyd::quantize_half_up(
                &pyd::mul28(&pyd::PyD::from_dec(book_value), &ddb_rate),
                places,
            );
            if !switched && sl_now >= ddb {
                switched = true;
            }
            if switched {
                sl_now
            } else {
                ddb
            }
        };
        if left < dep_amount {
            dep_amount = left;
        }
        if dep_amount <= Decimal::ZERO {
            break;
        }
        accumulated += dep_amount;
        book_value -= dep_amount;
        lines.push(ScheduleLine {
            period_number: i as i64 + 1,
            period_date,
            period_label,
            depreciation_amount: dep_amount,
            accumulated_depreciation: accumulated,
            book_value,
        });
    }
    lines
}

/// `_schedule_json`: the money as floats.
fn schedule_json(lines: &[ScheduleLine]) -> Vec<Map<String, Value>> {
    lines
        .iter()
        .map(|l| {
            let v = json!({
                "period_number": l.period_number,
                "period_date": date_iso(&l.period_date),
                "period_label": l.period_label,
                "depreciation_amount": dec_float(&l.depreciation_amount),
                "accumulated_depreciation": dec_float(&l.accumulated_depreciation),
                "book_value": dec_float(&l.book_value),
            });
            match v {
                Value::Object(m) => m,
                _ => unreachable!(),
            }
        })
        .collect()
}

fn schedule_inputs(l: &Loaded) -> (ScheduleAsset, Option<ScheduleCategory>) {
    (
        ScheduleAsset {
            depreciation_method: l.asset.depreciation_method.clone(),
            useful_life_months: l.asset.useful_life_months,
            purchase_price: l.asset.purchase_price,
            salvage_value: l.asset.salvage_value,
            purchase_date: l.asset.purchase_date,
        },
        l.category.as_ref().map(|c| ScheduleCategory {
            depreciation_method: c.depreciation_method.clone(),
            useful_life_months: c.useful_life_months,
        }),
    )
}

// ─── Serialisers ─────────────────────────────────────────────────────────────

fn uuid_opt(u: Option<Uuid>) -> Value {
    u.map(|x| Value::String(x.to_string()))
        .unwrap_or(Value::Null)
}

fn category_dict(c: &CategoryRow) -> Value {
    json!({
        "id": c.id.to_string(),
        "name": c.name,
        "description": c.description,
        "depreciation_method": c.depreciation_method,
        "useful_life_months": c.useful_life_months,
        "salvage_value_pct": float_or_zero(c.salvage_value_pct),
        "gl_asset_account_id": uuid_opt(c.gl_asset_account_id),
        "gl_depreciation_account_id": uuid_opt(c.gl_depreciation_account_id),
        "gl_accumulated_account_id": uuid_opt(c.gl_accumulated_account_id),
        "created_at": iso(&c.created_at),
    })
}

fn asset_dict(a: &AssetRow, category_name: Option<&str>) -> Map<String, Value> {
    let v = json!({
        "id": a.id.to_string(),
        "number": a.number,
        "name": a.name,
        "description": a.description,
        "category_id": uuid_opt(a.category_id),
        "category_name": category_name,
        "status": a.status,
        "purchase_date": date_iso(&a.purchase_date),
        "purchase_price": dec_float(&a.purchase_price),
        "currency": a.currency,
        "useful_life_months": a.useful_life_months,
        "salvage_value": float_or_zero(a.salvage_value),
        "depreciation_method": a.depreciation_method,
        "location": a.location,
        "serial_number": a.serial_number,
        "vendor_id": uuid_opt(a.vendor_id),
        "purchase_order_id": uuid_opt(a.purchase_order_id),
        "notes": a.notes,
        "created_at": iso(&a.created_at),
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

fn line_dict(l: &LineRow) -> Value {
    json!({
        "id": l.id.to_string(),
        "asset_id": l.asset_id.to_string(),
        "period_date": date_iso(&l.period_date),
        "period_label": l.period_label,
        "depreciation_amount": dec_float(&l.depreciation_amount),
        "accumulated_depreciation": dec_float(&l.accumulated_depreciation),
        "book_value": dec_float(&l.book_value),
        "journal_entry_id": uuid_opt(l.journal_entry_id),
        "status": l.status,
    })
}

fn disposal_dict(d: &DisposalRow) -> Map<String, Value> {
    let v = json!({
        "id": d.id.to_string(),
        "asset_id": d.asset_id.to_string(),
        "disposal_date": date_iso(&d.disposal_date),
        "disposal_type": d.disposal_type,
        "proceeds": float_or_zero(d.proceeds),
        "disposal_notes": d.disposal_notes,
        "journal_entry_id": uuid_opt(d.journal_entry_id),
        "created_at": iso(&d.created_at),
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

// ─── Category endpoints ──────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class CategoryCreate(BaseModel)`.
    pub struct CategoryCreate {
        pub name: String,
        pub description: Option<String> = None,
        pub depreciation_method: String = "straight_line",
        pub useful_life_months: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("0"))),
        pub salvage_value_pct: Decimal = Decimal::ZERO
            => v::decimal_c(v::DecC::new().ge("0").le("100").max_digits(5).decimal_places(2)),
        pub gl_asset_account_id: Option<String> = None,
        pub gl_depreciation_account_id: Option<String> = None,
        pub gl_accumulated_account_id: Option<String> = None,
    }
}

/// `GET /categories`.
pub async fn list_categories(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_asset_view(&user)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query_as::<_, CategoryRow>(&format!(
        "SELECT {CATEGORY_COLS} FROM asset_categories WHERE asset_categories.is_deleted = false \
         ORDER BY asset_categories.name"
    ))
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(category_dict).collect(),
    )))
}

/// `POST /categories` (201).
pub async fn create_category(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CategoryCreate>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    let gl_asset = pg_uuid_opt(body.gl_asset_account_id.as_deref())?;
    let gl_dep = pg_uuid_opt(body.gl_depreciation_account_id.as_deref())?;
    let gl_acc = pg_uuid_opt(body.gl_accumulated_account_id.as_deref())?;
    let row = sqlx::query_as::<_, CategoryRow>(&format!(
        "INSERT INTO asset_categories (name, description, depreciation_method, useful_life_months, \
         salvage_value_pct, gl_asset_account_id, gl_depreciation_account_id, gl_accumulated_account_id, \
         id, is_deleted) VALUES ($1, $2, $3, $4::integer, $5, $6, $7, $8, $9, false) RETURNING {}",
        CATEGORY_COLS.replace("asset_categories.", "")
    ))
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.depreciation_method)
    .bind(body.useful_life_months)
    .bind(body.salvage_value_pct)
    .bind(gl_asset)
    .bind(gl_dep)
    .bind(gl_acc)
    .bind(Uuid::new_v4())
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, category_dict(&row)))
}

async fn find_category(db: &mut PgConnection, category_id: &str) -> ApiResult<CategoryRow> {
    let id = pg_uuid(category_id)?;
    sqlx::query_as::<_, CategoryRow>(&format!(
        "SELECT {CATEGORY_COLS} FROM asset_categories WHERE asset_categories.id = $1 \
         AND asset_categories.is_deleted = false"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?
    .ok_or_else(|| ApiError::http(404, "Category not found"))
}

/// `PATCH /categories/{category_id}`: `CategoryCreate.model_dump(exclude_none=True)` onto the row (the
/// model's defaults included). The three GL accounts are the exception: a key sent as null clears that
/// account (an omitted key keeps it), so a wrong mapping can be removed.
pub async fn update_category(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(category_id): Path<String>,
    PyJson(sent): PyJson<crate::routers::finance::Sent<CategoryCreate>>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let body = &sent.model;
    let mut conn = db.conn().await?;
    let cat = find_category(&mut conn, &category_id).await?;
    let gl_asset = pg_uuid_opt(body.gl_asset_account_id.as_deref())?;
    let gl_dep = pg_uuid_opt(body.gl_depreciation_account_id.as_deref())?;
    let gl_acc = pg_uuid_opt(body.gl_accumulated_account_id.as_deref())?;
    let new_desc = body.description.clone().or(cat.description.clone());
    let new_life = body
        .useful_life_months
        .or(cat.useful_life_months.map(i64::from));
    let gl = |key: &str, given: Option<Uuid>, current: Option<Uuid>| {
        if sent.has(key) {
            given
        } else {
            current
        }
    };
    let new_asset = gl("gl_asset_account_id", gl_asset, cat.gl_asset_account_id);
    let new_dep = gl(
        "gl_depreciation_account_id",
        gl_dep,
        cat.gl_depreciation_account_id,
    );
    let new_acc = gl(
        "gl_accumulated_account_id",
        gl_acc,
        cat.gl_accumulated_account_id,
    );
    // SQLAlchemy only writes (and bumps updated_at) when a value changed.
    let changed = body.name != cat.name
        || new_desc != cat.description
        || Some(&body.depreciation_method) != cat.depreciation_method.as_ref()
        || new_life != cat.useful_life_months.map(i64::from)
        || Some(body.salvage_value_pct) != cat.salvage_value_pct
        || new_asset != cat.gl_asset_account_id
        || new_dep != cat.gl_depreciation_account_id
        || new_acc != cat.gl_accumulated_account_id;
    let row = if changed {
        sqlx::query_as::<_, CategoryRow>(&format!(
            "UPDATE asset_categories SET name = $1, description = $2, depreciation_method = $3, \
             useful_life_months = $4::integer, salvage_value_pct = $5, gl_asset_account_id = $6, \
             gl_depreciation_account_id = $7, gl_accumulated_account_id = $8, updated_at = now() \
             WHERE id = $9 RETURNING {}",
            CATEGORY_COLS.replace("asset_categories.", "")
        ))
        .bind(&body.name)
        .bind(&new_desc)
        .bind(&body.depreciation_method)
        .bind(new_life)
        .bind(body.salvage_value_pct)
        .bind(new_asset)
        .bind(new_dep)
        .bind(new_acc)
        .bind(cat.id)
        .fetch_one(&mut *conn)
        .await?
    } else {
        cat
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(category_dict(&row)))
}

/// `DELETE /categories/{category_id}` (204): soft delete.
pub async fn delete_category(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(category_id): Path<String>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    let cat = find_category(&mut conn, &category_id).await?;
    sqlx::query(
        "UPDATE asset_categories SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE id = $2",
    )
    .bind(now_micros())
    .bind(cat.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `datetime.now(timezone.utc)` (microseconds).
pub fn now_micros() -> DateTime<Utc> {
    let now = Utc::now();
    DateTime::from_timestamp_micros(now.timestamp_micros()).unwrap_or(now)
}

// ─── Asset endpoints ─────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class AssetCreate(BaseModel)`.
    pub struct AssetCreate {
        pub name: String,
        pub description: Option<String> = None,
        pub category_id: Option<String> = None,
        pub purchase_date: NaiveDate,
        pub purchase_price: Decimal
            => v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(2)),
        pub currency: String = "USD",
        pub useful_life_months: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("0"))),
        pub salvage_value: Decimal = Decimal::ZERO
            => v::decimal_c(v::DecC::new().ge("0").max_digits(18).decimal_places(2)),
        pub depreciation_method: Option<String> = None,
        pub location: Option<String> = None,
        pub serial_number: Option<String> = None,
        pub vendor_id: Option<String> = None,
        pub purchase_order_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class AssetUpdate(BaseModel)`.
    pub struct AssetUpdate {
        pub name: Option<String> = None,
        pub description: Option<String> = None,
        pub category_id: Option<String> = None,
        pub location: Option<String> = None,
        pub serial_number: Option<String> = None,
        pub notes: Option<String> = None,
        pub status: Option<String> = None,
    }
}

granary_core::py_model! {
    /// The query parameters of `list_assets`.
    pub struct ListAssetsQ {
        pub status: Option<String> = None,
        pub category_id: Option<String> = None,
        pub q: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `GET /assets`.
pub async fn list_assets(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListAssetsQ>,
) -> ApiResult<Response> {
    require_asset_view(&user)?;
    let mut conn = db.conn().await?;
    let mut sql =
        format!("SELECT {ASSET_COLS} FROM fixed_assets WHERE fixed_assets.is_deleted = false");
    let mut n = 0;
    let mut mark = || {
        n += 1;
        format!("${n}")
    };
    let status = truthy(&q.status).map(str::to_string);
    let category = match truthy(&q.category_id) {
        Some(c) => Some(pg_uuid(c)?),
        None => None,
    };
    let like = truthy(&q.q).map(|s| format!("%{s}%"));
    if status.is_some() {
        sql += &format!(" AND fixed_assets.status = {}", mark());
    }
    if category.is_some() {
        sql += &format!(" AND fixed_assets.category_id = {}", mark());
    }
    if like.is_some() {
        let m = mark();
        sql += &format!(
            " AND (fixed_assets.name ILIKE {m} OR fixed_assets.number ILIKE {m} OR fixed_assets.serial_number ILIKE {m})"
        );
    }
    let (ml, mo) = (mark(), mark());
    sql +=
        &format!(" ORDER BY fixed_assets.created_at DESC LIMIT {ml}::integer OFFSET {mo}::integer");
    let mut query = sqlx::query_as::<_, AssetRow>(&sql);
    if let Some(s) = &status {
        query = query.bind(s);
    }
    if let Some(c) = category {
        query = query.bind(c);
    }
    if let Some(l) = &like {
        query = query.bind(l);
    }
    let rows = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let names = category_names(&mut conn, rows.iter().filter_map(|a| a.category_id)).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|a| {
            let name = a
                .category_id
                .and_then(|c| names.get(&c))
                .map(String::as_str);
            Value::Object(asset_dict(a, name))
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `selectinload(FixedAsset.category)` for a page of assets: id -> name.
async fn category_names(
    db: &mut PgConnection,
    ids: impl Iterator<Item = Uuid>,
) -> ApiResult<HashMap<Uuid, String>> {
    let mut uniq: Vec<Uuid> = Vec::new();
    for id in ids {
        if !uniq.contains(&id) {
            uniq.push(id);
        }
    }
    if uniq.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT asset_categories.id, asset_categories.name FROM asset_categories WHERE asset_categories.id IN ({})",
        in_marks(1, uniq.len())
    );
    let mut q = sqlx::query_as::<_, (Uuid, String)>(&sql);
    for id in &uniq {
        q = q.bind(id);
    }
    Ok(q.fetch_all(&mut *db).await?.into_iter().collect())
}

/// `POST /assets` (201).
pub async fn create_asset(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<AssetCreate>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    if let Some(cid) = truthy(&body.category_id) {
        let id = pg_uuid(cid)?;
        let found: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM asset_categories WHERE id = $1 AND is_deleted = false")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?;
        if found.is_none() {
            return Err(ApiError::http(404, "Asset category not found"));
        }
    }
    let places = currency_places(&mut conn, Some(&body.currency)).await?;
    let purchase_price = quantize_money(body.purchase_price, places);
    let salvage_value = quantize_money(body.salvage_value, places);
    if salvage_value > purchase_price {
        return Err(ApiError::http(
            400,
            "Salvage value can't exceed the purchase price",
        ));
    }
    let functional = functional_currency(&mut conn).await?;
    let acquisition_rate = require_rate(
        &mut conn,
        &body.currency,
        &functional,
        Some(body.purchase_date),
    )
    .await?;
    let number = next_document_number(&mut conn, "asset").await?;
    // The flush binds every id column; a malformed one fails there.
    let category_id = pg_uuid_opt(body.category_id.as_deref())?;
    let vendor_id = pg_uuid_opt(body.vendor_id.as_deref())?;
    let po_id = pg_uuid_opt(body.purchase_order_id.as_deref())?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO fixed_assets (number, name, description, category_id, status, purchase_date, \
         purchase_price, currency, acquisition_rate, useful_life_months, salvage_value, depreciation_method, \
         location, serial_number, vendor_id, purchase_order_id, notes, created_by_id, id, is_deleted) \
         VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, $9::integer, $10, $11, $12, $13, $14, $15, $16, \
         $17, $18, false)",
    )
    .bind(&number)
    .bind(&body.name)
    .bind(&body.description)
    .bind(category_id)
    .bind(body.purchase_date)
    .bind(purchase_price)
    .bind(&body.currency)
    .bind(acquisition_rate)
    .bind(body.useful_life_months)
    .bind(salvage_value)
    .bind(&body.depreciation_method)
    .bind(&body.location)
    .bind(&body.serial_number)
    .bind(vendor_id)
    .bind(po_id)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let loaded = load_asset(&mut conn, &id.to_string(), false)
        .await?
        .ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!("No row was found when one was required"))
        })?;
    let name = loaded.category.as_ref().map(|c| c.name.as_str());
    Ok(resp::json_status(
        201,
        Value::Object(asset_dict(&loaded.asset, name)),
    ))
}

/// `GET /assets/{asset_id}`: with depreciation lines and disposals.
pub async fn get_asset(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(asset_id): Path<String>,
) -> ApiResult<Response> {
    require_asset_view(&user)?;
    let mut conn = db.conn().await?;
    let loaded = load_asset(&mut conn, &asset_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Asset not found"))?;
    let lines = load_lines(&mut conn, loaded.asset.id).await?;
    let disposals = sqlx::query_as::<_, DisposalRow>(&format!(
        "SELECT {DISPOSAL_COLS} FROM asset_disposals WHERE asset_disposals.asset_id IN ($1)"
    ))
    .bind(loaded.asset.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut d = asset_dict(
        &loaded.asset,
        loaded.category.as_ref().map(|c| c.name.as_str()),
    );
    d.insert(
        "depreciation_lines".into(),
        Value::Array(lines.iter().map(line_dict).collect()),
    );
    d.insert(
        "disposals".into(),
        Value::Array(
            disposals
                .iter()
                .map(|x| Value::Object(disposal_dict(x)))
                .collect(),
        ),
    );
    Ok(resp::json(Value::Object(d)))
}

/// `PATCH /assets/{asset_id}`: `AssetUpdate.model_dump(exclude_none=True)` onto the row.
pub async fn update_asset(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(asset_id): Path<String>,
    PyJson(body): PyJson<AssetUpdate>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    let loaded = load_asset(&mut conn, &asset_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Asset not found"))?;
    let a = &loaded.asset;
    let category_id = match &body.category_id {
        Some(c) => Some(pg_uuid(c)?),
        None => a.category_id,
    };
    let name = body.name.clone().unwrap_or_else(|| a.name.clone());
    let description = body.description.clone().or(a.description.clone());
    let location = body.location.clone().or(a.location.clone());
    let serial = body.serial_number.clone().or(a.serial_number.clone());
    let notes = body.notes.clone().or(a.notes.clone());
    let status = body.status.clone().or(a.status.clone());
    let changed = name != a.name
        || description != a.description
        || category_id != a.category_id
        || location != a.location
        || serial != a.serial_number
        || notes != a.notes
        || status != a.status;
    if changed {
        sqlx::query(
            "UPDATE fixed_assets SET name = $1, description = $2, category_id = $3, location = $4, \
             serial_number = $5, notes = $6, status = $7, updated_at = now() WHERE id = $8",
        )
        .bind(&name)
        .bind(&description)
        .bind(category_id)
        .bind(&location)
        .bind(&serial)
        .bind(&notes)
        .bind(&status)
        .bind(a.id)
        .execute(&mut *conn)
        .await?;
    }
    let id = a.id;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    // db.refresh(asset): the columns and the category it now points at.
    let asset = sqlx::query_as::<_, AssetRow>(&format!(
        "SELECT {ASSET_COLS} FROM fixed_assets WHERE fixed_assets.id = $1"
    ))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    let category_name = load_category(&mut conn, asset.category_id)
        .await?
        .map(|c| c.name);
    Ok(resp::json(Value::Object(asset_dict(
        &asset,
        category_name.as_deref(),
    ))))
}

/// `DELETE /assets/{asset_id}` (204): soft delete.
pub async fn delete_asset(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(asset_id): Path<String>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&asset_id)?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fixed_assets WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    if found.is_none() {
        return Err(ApiError::http(404, "Asset not found"));
    }
    sqlx::query(
        "UPDATE fixed_assets SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE id = $2",
    )
    .bind(now_micros())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Depreciation schedule ───────────────────────────────────────────────────

/// `GET /assets/{asset_id}/depreciation-schedule`: the calculated schedule with the stored lines'
/// status overlaid.
pub async fn get_depreciation_schedule(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(asset_id): Path<String>,
) -> ApiResult<Response> {
    require_asset_view(&user)?;
    let mut conn = db.conn().await?;
    let loaded = load_asset(&mut conn, &asset_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Asset not found"))?;
    let stored = load_lines(&mut conn, loaded.asset.id).await?;
    let places = currency_places(&mut conn, loaded.asset.currency.as_deref()).await?;
    let (sa, sc) = schedule_inputs(&loaded);
    let mut schedule = schedule_json(&calculate_schedule(&sa, sc.as_ref(), places));
    // {l.period_label: l for l in lines}: the last line of a label wins.
    let mut posted: HashMap<Option<String>, &LineRow> = HashMap::new();
    for l in &stored {
        posted.insert(l.period_label.clone(), l);
    }
    for entry in schedule.iter_mut() {
        let label = entry["period_label"].as_str().map(str::to_string);
        let found = posted.get(&label);
        entry.insert(
            "db_status".into(),
            match found {
                Some(l) => json!(l.status),
                None => json!("not_generated"),
            },
        );
        entry.insert(
            "db_line_id".into(),
            match found {
                Some(l) => json!(l.id.to_string()),
                None => Value::Null,
            },
        );
    }
    let a = &loaded.asset;
    let cat = loaded.category.as_ref();
    let method: Value = match truthy(&a.depreciation_method) {
        Some(m) => json!(m),
        None => match cat {
            Some(c) => json!(c.depreciation_method),
            None => json!("none"),
        },
    };
    let life: Value = match a.useful_life_months.filter(|n| *n != 0) {
        Some(n) => json!(n),
        None => match cat {
            Some(c) => json!(c.useful_life_months),
            None => json!(0),
        },
    };
    Ok(resp::json(json!({
        "asset_id": a.id.to_string(),
        "asset_name": a.name,
        "method": method,
        "purchase_price": dec_float(&a.purchase_price),
        "salvage_value": dec_float(&D(a.salvage_value)),
        "useful_life_months": life,
        "schedule": schedule.into_iter().map(Value::Object).collect::<Vec<_>>(),
    })))
}

/// `POST /assets/{asset_id}/generate-schedule` (201): stores the periods not stored yet.
pub async fn generate_schedule(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(asset_id): Path<String>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    let loaded = load_asset(&mut conn, &asset_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Asset not found"))?;
    let stored = load_lines(&mut conn, loaded.asset.id).await?;
    let existing: Vec<Option<String>> = stored.iter().map(|l| l.period_label.clone()).collect();
    let places = currency_places(&mut conn, loaded.asset.currency.as_deref()).await?;
    let (sa, sc) = schedule_inputs(&loaded);
    let schedule = calculate_schedule(&sa, sc.as_ref(), places);
    let mut created = 0;
    for entry in &schedule {
        if existing.contains(&Some(entry.period_label.clone())) {
            continue;
        }
        sqlx::query(
            "INSERT INTO asset_depreciation_lines (asset_id, period_date, period_label, depreciation_amount, \
             accumulated_depreciation, book_value, status, id) VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)",
        )
        .bind(loaded.asset.id)
        .bind(entry.period_date)
        .bind(&entry.period_label)
        .bind(entry.depreciation_amount)
        .bind(entry.accumulated_depreciation)
        .bind(entry.book_value)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
        created += 1;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"created": created, "total_periods": schedule.len()}),
    ))
}

fn asset_doc(a: &AssetRow) -> AssetDoc {
    AssetDoc {
        number: a.number.clone(),
        name: a.name.clone(),
        currency: a.currency.clone(),
        purchase_price: a.purchase_price,
        acquisition_rate: a.acquisition_rate,
    }
}

fn category_doc(c: &CategoryRow) -> AssetCategoryDoc {
    AssetCategoryDoc {
        gl_asset_account_id: c.gl_asset_account_id,
        gl_depreciation_account_id: c.gl_depreciation_account_id,
        gl_accumulated_account_id: c.gl_accumulated_account_id,
    }
}

/// `POST /assets/{asset_id}/post-depreciation`: posts the next pending line to the GL.
pub async fn post_depreciation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(asset_id): Path<String>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    let loaded = load_asset(&mut conn, &asset_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Asset not found"))?;
    let a = &loaded.asset;
    load_lines(&mut conn, a.id).await?;
    // The next pending line, locked: a second caller waits, then sees it posted and takes the next.
    let pending = sqlx::query_as::<_, LineRow>(&format!(
        "SELECT {LINE_COLS} FROM asset_depreciation_lines WHERE asset_depreciation_lines.asset_id = $1 \
         AND asset_depreciation_lines.status = 'pending' ORDER BY asset_depreciation_lines.period_date \
         LIMIT 1 FOR UPDATE"
    ))
    .bind(a.id)
    .fetch_optional(&mut *conn)
    .await?
    .filter(|l| l.status.as_deref() == Some("pending"));
    let Some(pending) = pending else {
        return Err(ApiError::http(
            400,
            "No pending depreciation periods found. Generate the schedule first.",
        ));
    };
    if a.status.as_deref() == Some("disposed") {
        return Err(ApiError::http(
            400,
            "A disposed asset is no longer depreciated",
        ));
    }
    if pending.period_date > today_local(&mut conn).await? {
        return Err(ApiError::http(
            400,
            format!(
                "{} hasn't started yet: depreciation is posted for past periods only",
                pending.period_label.as_deref().unwrap_or("None")
            ),
        ));
    }
    let cat = match &loaded.category {
        Some(c) if c.gl_depreciation_account_id.is_some() && c.gl_accumulated_account_id.is_some() => c,
        _ => {
            return Err(ApiError::http(
                400,
                "Set the depreciation and accumulated depreciation accounts on the asset's category first",
            ))
        }
    };
    // gl_posting takes the stored row itself.
    let line_row = sqlx::query_as("SELECT * FROM asset_depreciation_lines WHERE id = $1")
        .bind(pending.id)
        .fetch_one(&mut *conn)
        .await?;
    let je = gl_posting::post_depreciation(
        &mut conn,
        &asset_doc(a),
        &line_row,
        &category_doc(cat),
        Some(user.id),
    )
    .await?;
    let je_id = je.map(|j| j.id);
    sqlx::query(
        "UPDATE asset_depreciation_lines SET status = 'posted', journal_entry_id = COALESCE($1, journal_entry_id), \
         updated_at = now() WHERE id = $2",
    )
    .bind(je_id)
    .bind(pending.id)
    .execute(&mut *conn)
    .await?;
    if pending.book_value <= D(a.salvage_value) && a.status.as_deref() != Some("fully_depreciated")
    {
        sqlx::query(
            "UPDATE fixed_assets SET status = 'fully_depreciated', updated_at = now() WHERE id = $1",
        )
        .bind(a.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "posted_period": pending.period_label,
        "depreciation_amount": dec_float(&pending.depreciation_amount),
        "journal_entry_id": je_id.map(|j| j.to_string()),
        "gl_entries_created": je_id.is_some(),
    })))
}

// ─── Disposal ────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class DisposalCreate(BaseModel)`.
    pub struct DisposalCreate {
        pub disposal_date: NaiveDate,
        /// sold | scrapped | donated
        pub disposal_type: String,
        pub proceeds: Decimal = Decimal::ZERO
            => v::decimal_c(v::DecC::new().ge("0").max_digits(18).decimal_places(2)),
        pub disposal_notes: Option<String> = None,
        /// where proceeds arrived; empty = the bank_default GL role
        pub bank_account_id: Option<String> = None,
    }
}

/// `POST /assets/{asset_id}/dispose` (201).
pub async fn dispose_asset(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(asset_id): Path<String>,
    PyJson(body): PyJson<DisposalCreate>,
) -> ApiResult<Response> {
    require_asset_manage(&user)?;
    let mut conn = db.conn().await?;
    let loaded = load_asset(&mut conn, &asset_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Asset not found"))?;
    let a = &loaded.asset;
    let lines = load_lines(&mut conn, a.id).await?;
    if a.status.as_deref() == Some("disposed") {
        return Err(ApiError::http(400, "Asset is already disposed"));
    }
    let places = currency_places(&mut conn, a.currency.as_deref()).await?;
    let total_acc_dep: Decimal = lines
        .iter()
        .filter(|l| l.status.as_deref() == Some("posted"))
        .map(|l| l.depreciation_amount)
        .sum();
    let net_book_value = a.purchase_price - total_acc_dep;
    let proceeds = quantize_money(body.proceeds, places);
    let bank_account_id = pg_uuid_opt(truthy(&body.bank_account_id))?;
    let disposal_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO asset_disposals (asset_id, disposal_date, disposal_type, proceeds, bank_account_id, \
         disposal_notes, created_by_id, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(a.id)
    .bind(body.disposal_date)
    .bind(&body.disposal_type)
    .bind(proceeds)
    .bind(bank_account_id)
    .bind(&body.disposal_notes)
    .bind(user.id)
    .bind(disposal_id)
    .execute(&mut *conn)
    .await?;
    if a.status.as_deref() != Some("disposed") {
        sqlx::query(
            "UPDATE fixed_assets SET status = 'disposed', updated_at = now() WHERE id = $1",
        )
        .bind(a.id)
        .execute(&mut *conn)
        .await?;
    }
    // Periods after the disposal will never be depreciated.
    for l in lines
        .iter()
        .filter(|l| l.status.as_deref() == Some("pending"))
    {
        sqlx::query(
            "UPDATE asset_depreciation_lines SET status = 'cancelled', updated_at = now() WHERE id = $1",
        )
        .bind(l.id)
        .execute(&mut *conn)
        .await?;
    }
    if gl_posting::autopost_enabled(&mut conn, Some(body.disposal_date)).await? {
        let cat = match &loaded.category {
            Some(c) if c.gl_asset_account_id.is_some() && c.gl_accumulated_account_id.is_some() => c,
            _ => {
                return Err(ApiError::http(
                    400,
                    "Set the asset and accumulated depreciation accounts on the asset's category before disposing of it",
                ))
            }
        };
        let functional = functional_currency(&mut conn).await?;
        let proceeds_rate = require_rate(
            &mut conn,
            a.currency.as_deref().unwrap_or("None"),
            &functional,
            Some(body.disposal_date),
        )
        .await?;
        let doc = DisposalDoc {
            id: disposal_id,
            disposal_date: body.disposal_date,
            disposal_type: body.disposal_type.clone(),
            proceeds: Some(proceeds),
            bank_account_id,
        };
        let je = gl_posting::post_asset_disposal(
            &mut conn,
            &asset_doc(a),
            &doc,
            &category_doc(cat),
            total_acc_dep,
            Some(proceeds_rate),
            Some(user.id),
        )
        .await?;
        if let Some(je) = je {
            sqlx::query(
                "UPDATE asset_disposals SET journal_entry_id = $1, updated_at = now() WHERE id = $2",
            )
            .bind(je.id)
            .bind(disposal_id)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let disposal = sqlx::query_as::<_, DisposalRow>(&format!(
        "SELECT {DISPOSAL_COLS} FROM asset_disposals WHERE asset_disposals.id = $1"
    ))
    .bind(disposal_id)
    .fetch_one(&mut *conn)
    .await?;
    let mut out = disposal_dict(&disposal);
    out.insert(
        "net_book_value_at_disposal".into(),
        dec_float(&net_book_value),
    );
    out.insert("gain_loss".into(), dec_float(&(proceeds - net_book_value)));
    Ok(resp::json_status(201, Value::Object(out)))
}

/// Python's default decimal context for the schedule maths: a quotient or product rounded half-even to
/// 28 significant digits (a rate such as 2/48 needs 29 decimal places, more than rust_decimal holds),
/// then `quantize(..., ROUND_HALF_UP)`. Non-negative operands only (what the schedule computes).
pub mod pyd {
    use rust_decimal::Decimal;

    const PREC: usize = 28;

    /// digits (no leading zeros; empty = 0) x 10^exp.
    #[derive(Clone, Debug, PartialEq)]
    pub struct PyD {
        pub digits: Vec<u8>,
        pub exp: i64,
    }

    impl PyD {
        pub fn from_dec(d: Decimal) -> PyD {
            let m = d.mantissa().unsigned_abs();
            let digits = if m == 0 {
                Vec::new()
            } else {
                m.to_string().bytes().map(|c| c - b'0').collect()
            };
            PyD {
                digits,
                exp: -(d.scale() as i64),
            }
        }
    }

    /// Round to 28 significant digits, half-even (`sticky`: nonzero digits beyond those given).
    fn round28(mut digits: Vec<u8>, mut exp: i64, sticky: bool) -> PyD {
        let first = digits.iter().position(|d| *d != 0).unwrap_or(digits.len());
        digits.drain(..first);
        if digits.len() <= PREC {
            return PyD { digits, exp };
        }
        let drop = digits.len() - PREC;
        let guard = digits[PREC];
        let rest = sticky || digits[PREC + 1..].iter().any(|d| *d != 0);
        digits.truncate(PREC);
        exp += drop as i64;
        let odd = digits[PREC - 1] % 2 == 1;
        if guard > 5 || (guard == 5 && (rest || odd)) {
            let mut i = digits.len();
            loop {
                if i == 0 {
                    digits.insert(0, 1);
                    break;
                }
                i -= 1;
                if digits[i] == 9 {
                    digits[i] = 0;
                } else {
                    digits[i] += 1;
                    break;
                }
            }
            if digits.len() > PREC {
                digits.pop();
                exp += 1;
            }
        }
        PyD { digits, exp }
    }

    /// `a / b` in Python's context.
    pub fn div28(a: Decimal, b: Decimal) -> PyD {
        let ma = a.mantissa().unsigned_abs();
        let mb = b.mantissa().unsigned_abs();
        if ma == 0 || mb == 0 {
            return PyD {
                digits: Vec::new(),
                exp: 0,
            };
        }
        let mut exp: i64 = b.scale() as i64 - a.scale() as i64;
        let q = ma / mb;
        let mut r = ma % mb;
        let mut digits: Vec<u8> = if q == 0 {
            Vec::new()
        } else {
            q.to_string().bytes().map(|c| c - b'0').collect()
        };
        let sig = |d: &Vec<u8>| d.iter().skip_while(|x| **x == 0).count();
        while r != 0 && sig(&digits) < PREC + 2 {
            r *= 10;
            digits.push((r / mb) as u8);
            r %= mb;
            exp -= 1;
        }
        round28(digits, exp, r != 0)
    }

    /// `a * b` in Python's context.
    pub fn mul28(a: &PyD, b: &PyD) -> PyD {
        if a.digits.is_empty() || b.digits.is_empty() {
            return PyD {
                digits: Vec::new(),
                exp: 0,
            };
        }
        let mut acc = vec![0u32; a.digits.len() + b.digits.len()];
        for (i, x) in a.digits.iter().rev().enumerate() {
            for (j, y) in b.digits.iter().rev().enumerate() {
                acc[i + j] += (*x as u32) * (*y as u32);
            }
        }
        let mut carry = 0u32;
        for v in acc.iter_mut() {
            let t = *v + carry;
            *v = t % 10;
            carry = t / 10;
        }
        while carry > 0 {
            acc.push(carry % 10);
            carry /= 10;
        }
        let digits: Vec<u8> = acc.iter().rev().map(|d| *d as u8).collect();
        round28(digits, a.exp + b.exp, false)
    }

    /// `x.quantize(Decimal(1).scaleb(-places), rounding=ROUND_HALF_UP)`.
    pub fn quantize_half_up(x: &PyD, places: u32) -> Decimal {
        quantize(x, places, false)
    }

    /// `x.quantize(Decimal(1).scaleb(-places))` with ROUND_HALF_EVEN (Python's `round(x, places)`).
    pub fn quantize_half_even(x: &PyD, places: u32) -> Decimal {
        quantize(x, places, true)
    }

    fn quantize(x: &PyD, places: u32, half_even: bool) -> Decimal {
        let target = -(places as i64);
        let mut m: i128 = 0;
        if x.exp >= target {
            for d in &x.digits {
                m = m * 10 + *d as i128;
            }
            for _ in 0..(x.exp - target) {
                m *= 10;
            }
        } else {
            let drop = (target - x.exp) as usize;
            let keep = x.digits.len().saturating_sub(drop);
            for d in &x.digits[..keep] {
                m = m * 10 + *d as i128;
            }
            let (guard, rest) = if drop <= x.digits.len() {
                (x.digits[keep], x.digits[keep + 1..].iter().any(|d| *d != 0))
            } else {
                (0, true)
            };
            let up = if half_even {
                guard > 5 || (guard == 5 && (rest || m % 2 == 1))
            } else {
                guard >= 5
            };
            if up {
                m += 1;
            }
        }
        Decimal::from_i128_with_scale(m, places)
    }
}
