//! backend/app/routers/product_catalog.py, mounted at `/api/v1/catalog` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! The shared product-line catalog (`/product-lines`): anyone signed in reads it; the Owner and the
//! Executive role edit it (`require_catalog_manager`).
//!
//! Python quirks kept: PATCH applies `model_dump(exclude_unset=True)`, so an explicit `null` is written
//! (a null `is_active` or `name` breaks NOT NULL at the commit: the 409 IntegrityError handler); ids are
//! `str` compared with the UUID column (a malformed one is the central 422).

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{require_catalog_manager, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult, AppState};

use super::assets::now_micros;
use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/catalog";

/// The routes of `router` in product_catalog.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/product-lines"),
            get(list_product_lines).post(create_product_line),
        )
        .route(
            &at(PREFIX, "/product-lines/{line_id}"),
            patch(update_product_line).delete(delete_product_line),
        )
}

#[derive(sqlx::FromRow, Clone, Debug)]
struct ProductLineRow {
    id: Uuid,
    code: String,
    name: String,
    category: Option<String>,
    color: Option<String>,
    description: Option<String>,
    is_active: bool,
    sort_order: Option<i32>,
}

const COLS: &str = "product_lines.id, product_lines.code, product_lines.name, product_lines.category, \
     product_lines.color, product_lines.description, product_lines.is_active, product_lines.sort_order";

fn line_dict(p: &ProductLineRow) -> Value {
    json!({
        "id": p.id.to_string(),
        "code": p.code,
        "name": p.name,
        "category": p.category,
        "color": p.color,
        "description": p.description,
        "is_active": p.is_active,
        "sort_order": p.sort_order,
    })
}

granary_core::py_model! {
    /// The query parameters of `list_product_lines`.
    pub struct ListQ {
        pub include_inactive: bool = false,
    }
}

/// `GET /product-lines`.
pub async fn list_product_lines(
    CurrentUser(_user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let filter = if q.include_inactive {
        ""
    } else {
        " AND product_lines.is_active = true"
    };
    let rows = sqlx::query_as::<_, ProductLineRow>(&format!(
        "SELECT {COLS} FROM product_lines WHERE product_lines.is_deleted = false{filter} \
         ORDER BY product_lines.sort_order ASC, product_lines.name ASC"
    ))
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(line_dict).collect(),
    )))
}

granary_core::py_model! {
    /// `class ProductLineCreate(BaseModel)`.
    pub struct ProductLineCreate {
        pub code: String,
        pub name: String,
        pub category: String = "software",
        pub color: String = "#6366f1",
        pub description: Option<String> = None,
        pub is_active: bool = true,
        pub sort_order: i64 = 0,
    }
}

/// Python `str.strip()` (Unicode whitespace).
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `POST /product-lines` (201).
pub async fn create_product_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ProductLineCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let code = py_strip(&body.code).to_lowercase().replace(' ', "_");
    if code.is_empty() {
        return Err(ApiError::http(422, "Code is required"));
    }
    // scalar_one_or_none() on (code, is_deleted=false): code is unique, so at most one row.
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM product_lines WHERE product_lines.code = $1 AND product_lines.is_deleted = false",
    )
    .bind(&code)
    .fetch_optional(&mut *conn)
    .await?;
    if existing.is_some() {
        return Err(ApiError::http(
            409,
            format!("Product line '{code}' already exists"),
        ));
    }
    let row = sqlx::query_as::<_, ProductLineRow>(&format!(
        "INSERT INTO product_lines (code, name, category, color, description, is_active, sort_order, id, \
         is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7::integer, $8, false) RETURNING {}",
        COLS.replace("product_lines.", "")
    ))
    .bind(&code)
    .bind(py_strip(&body.name))
    .bind(&body.category)
    .bind(&body.color)
    .bind(&body.description)
    .bind(body.is_active)
    .bind(body.sort_order)
    .bind(Uuid::new_v4())
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, line_dict(&row)))
}

/// A field that remembers whether it was sent (`exclude_unset`): None = not sent, Some(None) = null.
#[derive(Clone, Debug)]
pub struct Sent<T>(pub Option<Option<T>>);

impl<T> Default for Sent<T> {
    fn default() -> Self {
        Sent(None)
    }
}

impl<T: Validate> Validate for Sent<T> {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        <Option<T> as Validate>::validate(v, cx).map(|x| Sent(Some(x)))
    }
}

granary_core::py_model! {
    /// `class ProductLineUpdate(BaseModel)`, with `exclude_unset` tracking.
    pub struct ProductLineUpdate {
        pub name: Sent<String> = Sent::default(),
        pub category: Sent<String> = Sent::default(),
        pub color: Sent<String> = Sent::default(),
        pub description: Sent<String> = Sent::default(),
        pub is_active: Sent<bool> = Sent::default(),
        pub sort_order: Sent<i64> = Sent::default(),
    }
}

async fn find_line(db: &mut PgConnection, line_id: &str) -> ApiResult<ProductLineRow> {
    let id = pg_uuid(line_id)?;
    sqlx::query_as::<_, ProductLineRow>(&format!(
        "SELECT {COLS} FROM product_lines WHERE product_lines.id = $1 AND product_lines.is_deleted = false"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?
    .ok_or_else(|| ApiError::http(404, "Product line not found"))
}

/// `PATCH /product-lines/{line_id}`: the fields sent, nulls included.
pub async fn update_product_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(line_id): Path<String>,
    PyJson(body): PyJson<ProductLineUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let p = find_line(&mut conn, &line_id).await?;
    let name: Option<String> = match &body.name.0 {
        Some(v) => v.clone(),
        None => Some(p.name.clone()),
    };
    let category = body.category.0.clone().unwrap_or(p.category.clone());
    let color = body.color.0.clone().unwrap_or(p.color.clone());
    let description = body.description.0.clone().unwrap_or(p.description.clone());
    let is_active: Option<bool> = body.is_active.0.unwrap_or(Some(p.is_active));
    let sort_order: Option<i64> = body.sort_order.0.unwrap_or(p.sort_order.map(i64::from));
    let changed = name.as_ref() != Some(&p.name)
        || category != p.category
        || color != p.color
        || description != p.description
        || is_active != Some(p.is_active)
        || sort_order != p.sort_order.map(i64::from);
    let row = if changed {
        sqlx::query_as::<_, ProductLineRow>(&format!(
            "UPDATE product_lines SET name = $1, category = $2, color = $3, description = $4, is_active = $5, \
             sort_order = $6::integer, updated_at = now() WHERE id = $7 RETURNING {}",
            COLS.replace("product_lines.", "")
        ))
        .bind(&name)
        .bind(&category)
        .bind(&color)
        .bind(&description)
        .bind(is_active)
        .bind(sort_order)
        .bind(p.id)
        .fetch_one(&mut *conn)
        .await?
    } else {
        p
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(line_dict(&row)))
}

/// `DELETE /product-lines/{line_id}` (204): soft delete.
pub async fn delete_product_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(line_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let p = find_line(&mut conn, &line_id).await?;
    sqlx::query(
        "UPDATE product_lines SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE id = $2",
    )
    .bind(now_micros())
    .bind(p.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
