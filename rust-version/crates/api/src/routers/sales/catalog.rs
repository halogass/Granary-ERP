//! sales.py "Product Catalog" and "Pricing preview" sections.
//!
//! A catalog item says how a SKU is sold; its prices are effective-dated rows per currency
//! (catalog_prices) and its volume discounts are tiers (catalog_volume_tiers). Only the Owner / Executive
//! edit any of it (require_catalog_manager).
//!
//! Inserts follow SQLAlchemy's ORM rules: an attribute left at None takes the column's Python default
//! (`unit` "Unit", `modules` [] ...) or NULL, except JSONB columns, where None is the JSON `null`.

use std::collections::HashSet;

use axum::extract::Path;
use axum::response::Response;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{can_manage_catalog, require_catalog_manager, AuthUser, CurrentUser};
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::validation::v::{self, DecC};
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::sales::{CatalogPrice, CatalogVolumeTier, ProductCatalog};
use granary_services::catalog::writes;
// 23 Oct: the request models moved with the writes (granary_services::catalog::writes)
pub use granary_services::catalog::writes::{
    PriceRowBody, PriceRowPatch, ProductCreate, ProductFields, TierRow, TiersBody,
};
use granary_services::pricing;

use super::{commit, WithSet, MONEY_MAX};
use crate::resp::{self, date_iso, date_iso_opt, dec_float, dec_float_opt};

/// `_current_prices(p, today)`: today's own list price per currency (rows sorted by valid_from).
fn current_prices(rows: &[CatalogPrice], today: NaiveDate) -> Value {
    let mut sorted: Vec<&CatalogPrice> = rows.iter().collect();
    sorted.sort_by_key(|r| r.valid_from);
    let mut out = Map::new();
    for r in sorted {
        if r.valid_from <= today && r.valid_to.is_none_or(|t| t >= today) {
            out.insert(r.currency.clone(), dec_float(&r.amount));
        }
    }
    Value::Object(out)
}

/// `_product_json(p, today, can_see_cost)`.
fn product_json(
    p: &ProductCatalog,
    prices: &[CatalogPrice],
    tiers: &[CatalogVolumeTier],
    today: NaiveDate,
    can_see_cost: bool,
) -> Value {
    let mut by_cur: Vec<&CatalogPrice> = prices.iter().collect();
    by_cur.sort_by(|a, b| (&a.currency, a.valid_from).cmp(&(&b.currency, b.valid_from)));
    let mut by_qty: Vec<&CatalogVolumeTier> = tiers.iter().collect();
    by_qty.sort_by_key(|t| t.min_qty);
    let mut out = json!({
        "id": p.id.to_string(), "sku": p.sku, "name": p.name, "short_name": p.short_name,
        "description": p.description, "category": p.category, "family": p.family,
        "product_type": p.product_type, "unit": p.unit, "sale_model": p.sale_model,
        "price_metric": p.price_metric, "rate_period_months": p.rate_period_months,
        "billing_interval_months": p.billing_interval_months,
        "min_quantity": dec_float_opt(p.min_quantity.as_ref()),
        "min_term_months": p.min_term_months,
        "unit_equivalents": dec_float_opt(p.unit_equivalents.as_ref()),
        "modules": p.modules.clone().unwrap_or_default(),
    });
    let rest = json!({
        "included_support_months": p.included_support_months, "pricing_mode": p.pricing_mode,
        "percent_rate": dec_float_opt(p.percent_rate.as_ref()),
        "addon_for": p.addon_for.clone().unwrap_or_default(),
        "whitelabel_free_min_qty": p.whitelabel_free_min_qty,
        "whitelabel_rule": p.whitelabel_rule.clone().unwrap_or(Value::Null),
        "is_whitelabel": p.is_whitelabel, "terms_note": p.terms_note,
        "is_active": p.is_active, "is_orderable": p.is_orderable, "image_url": p.image_url,
        "product_line_id": p.product_line_id.map(|u| u.to_string()),
        "price_rows": by_cur.iter().map(|r| json!({
            "id": r.id.to_string(), "currency": r.currency, "amount": dec_float(&r.amount),
            "valid_from": date_iso(&r.valid_from), "valid_to": date_iso_opt(r.valid_to.as_ref()),
        })).collect::<Vec<_>>(),
        "volume_tiers": by_qty.iter().map(|t| json!({
            "id": t.id.to_string(), "min_qty": dec_float(&t.min_qty),
            "max_qty": dec_float_opt(t.max_qty.as_ref()), "discount_pct": dec_float(&t.discount_pct),
        })).collect::<Vec<_>>(),
        "current_prices": current_prices(prices, today),
    });
    if let (Value::Object(o), Value::Object(r)) = (&mut out, rest) {
        o.extend(r);
    }
    // the segment (services::segment) and whether the price is locked or still proposed
    out["segment"] = json!(p.segment);
    out["price_status"] = json!(p.price_status.clone().unwrap_or_else(|| "locked".into()));
    if can_see_cost {
        out["cost_price"] = dec_float_opt(p.cost_price.as_ref());
    }
    out
}

/// `_require_catalog_view(current_user)`.
fn require_catalog_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin
        || user.perms.contains("sales:view_catalog")
        || user.perms.contains("sales:manage_catalog")
    {
        Ok(())
    } else {
        Err(ApiError::http(
            403,
            "Permission required: sales:view_catalog",
        ))
    }
}

/// `selectinload(price_rows)` / `selectinload(volume_tiers)` for a set of products (relationship order_by).
async fn load_children(
    db: &mut PgConnection,
    ids: &[Uuid],
) -> ApiResult<(Vec<CatalogPrice>, Vec<CatalogVolumeTier>)> {
    if ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let marks: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "SELECT * FROM catalog_prices WHERE catalog_prices.product_id IN ({}) ORDER BY catalog_prices.valid_from",
        marks.join(", ")
    );
    let mut q = sqlx::query_as::<_, CatalogPrice>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    let prices = q.fetch_all(&mut *db).await?;
    let sql = format!(
        "SELECT * FROM catalog_volume_tiers WHERE catalog_volume_tiers.product_id IN ({}) \
         ORDER BY catalog_volume_tiers.min_qty",
        marks.join(", ")
    );
    let mut q = sqlx::query_as::<_, CatalogVolumeTier>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    let tiers = q.fetch_all(&mut *db).await?;
    Ok((prices, tiers))
}

/// `_load_product(db, product_id)`: 400 for a malformed id, 404 for a missing or deleted product.
async fn load_product(db: &mut PgConnection, product_id: &str) -> ApiResult<ProductCatalog> {
    writes::load_product(db, product_id).await
}

granary_core::py_model! {
    /// The query parameters of `GET /catalog`.
    pub struct CatalogQuery {
        pub q: Option<String> = None,
        pub family: Option<String> = None,
        pub product_type: Option<String> = None,
        pub include_retired: bool = false,
        pub limit: i64 = 500,
        pub offset: i64 = 0,
    }
}

/// `GET /catalog`: catalog items with their price rows, tiers and today's prices. Retired items (inactive
/// or not orderable) only with include_retired; cost_price only for catalog managers.
pub async fn list_catalog(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(p): PyQuery<CatalogQuery>,
) -> ApiResult<Response> {
    require_catalog_view(&user)?;
    let mut conn = db.conn().await?;
    let mut sql =
        String::from("SELECT * FROM product_catalog WHERE product_catalog.is_deleted = false");
    let mut binds: Vec<String> = Vec::new();
    if let Some(q) = p.q.as_deref().filter(|s| !s.is_empty()) {
        binds.push(format!("%{}%", granary_compat::text::utils::escape_like(q)));
        let n = binds.len();
        sql.push_str(&format!(
            " AND (product_catalog.name ILIKE ${n} ESCAPE '\\' OR product_catalog.sku ILIKE ${n} ESCAPE '\\')"
        ));
    }
    if let Some(f) = p.family.as_deref().filter(|s| !s.is_empty()) {
        binds.push(f.to_string());
        sql.push_str(&format!(" AND product_catalog.family = ${}", binds.len()));
    }
    if let Some(t) = p.product_type.as_deref().filter(|s| !s.is_empty()) {
        binds.push(t.to_string());
        sql.push_str(&format!(
            " AND product_catalog.product_type = ${}",
            binds.len()
        ));
    }
    if !p.include_retired {
        sql.push_str(
            " AND product_catalog.is_active = true AND product_catalog.is_orderable = true",
        );
    }
    let limit = p.limit.clamp(1, 1000);
    sql.push_str(&format!(
        " ORDER BY product_catalog.family NULLS LAST, product_catalog.name LIMIT ${} OFFSET ${}",
        binds.len() + 1,
        binds.len() + 2
    ));
    let mut q = sqlx::query_as::<_, ProductCatalog>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q.bind(limit).bind(p.offset).fetch_all(&mut *conn).await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let (prices, tiers) = load_children(&mut conn, &ids).await?;
    let today = today_local(&mut conn).await?;
    let can_see_cost = can_manage_catalog(&mut conn, &user).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let pr: Vec<CatalogPrice> = prices
                .iter()
                .filter(|x| x.product_id == r.id)
                .cloned()
                .collect();
            let ti: Vec<CatalogVolumeTier> = tiers
                .iter()
                .filter(|x| x.product_id == r.id)
                .cloned()
                .collect();
            product_json(r, &pr, &ti, today, can_see_cost)
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /catalog` (201).
pub async fn create_product(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ProductCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    // 23 Oct: the write is granary_services::catalog::writes (shared with Nary's catalog.change)
    let id = writes::create_product(&mut conn, user.id, &body).await?;
    commit(&mut conn).await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "sku": body.sku, "name": body.name}),
    ))
}

/// `GET /catalog/access`: whether the viewer may edit the catalog (Owner / Executive only).
pub async fn catalog_access(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let can = can_manage_catalog(&mut conn, &user).await?;
    Ok(resp::json(json!({"can_edit": can})))
}

/// `GET /catalog/{product_id}`.
pub async fn get_product(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
) -> ApiResult<Response> {
    get_product_impl(user, db, product_id).await
}

async fn get_product_impl(user: AuthUser, db: Db, product_id: String) -> ApiResult<Response> {
    require_catalog_view(&user)?;
    let mut conn = db.conn().await?;
    let p = load_product(&mut conn, &product_id).await?;
    let (prices, tiers) = load_children(&mut conn, &[p.id]).await?;
    let today = today_local(&mut conn).await?;
    let can = can_manage_catalog(&mut conn, &user).await?;
    Ok(resp::json(product_json(&p, &prices, &tiers, today, can)))
}

/// `PATCH /catalog/{product_id}`.
pub async fn update_product(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
    PyJson(body): PyJson<WithSet<ProductFields>>,
) -> ApiResult<Response> {
    update_product_impl(user, db, product_id, body).await
}

/// `PATCH /catalog/access` reaches `update_product` in Starlette.
pub async fn update_product_access(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<WithSet<ProductFields>>,
) -> ApiResult<Response> {
    update_product_impl(user, db, "access".into(), body).await
}

async fn update_product_impl(
    user: AuthUser,
    db: Db,
    product_id: String,
    body: WithSet<ProductFields>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let product = load_product(&mut conn, &product_id).await?;
    writes::update_product(&mut conn, &product, &body.model, &body.set).await?;
    commit(&mut conn).await?;
    Ok(resp::json(json!({"id": product.id.to_string()})))
}

/// `DELETE /catalog/{product_id}` (204): a soft delete.
pub async fn delete_product(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
) -> ApiResult<Response> {
    delete_product_impl(user, db, product_id).await
}

/// `DELETE /catalog/access` reaches `delete_product` in Starlette.
pub async fn delete_product_access(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    delete_product_impl(user, db, "access".into()).await
}

async fn delete_product_impl(user: AuthUser, db: Db, product_id: String) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let product = load_product(&mut conn, &product_id).await?;
    writes::delete_product(&mut conn, product.id).await?;
    commit(&mut conn).await?;
    Ok(resp::no_content())
}

/// `_price_row_json(r)` of the row object as the session holds it.
fn price_row_json(
    id: Uuid,
    product_id: Uuid,
    currency: &str,
    amount: &Decimal,
    valid_from: &NaiveDate,
    valid_to: Option<&NaiveDate>,
) -> Value {
    json!({"id": id.to_string(), "product_id": product_id.to_string(), "currency": currency,
           "amount": dec_float(amount), "valid_from": date_iso(valid_from),
           "valid_to": date_iso_opt(valid_to)})
}

/// `POST /catalog/{product_id}/prices` (201): a list price in one currency from a date.
pub async fn add_price_row(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
    PyJson(body): PyJson<PriceRowBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let product = load_product(&mut conn, &product_id).await?;
    let (id, code) = writes::add_price_row(&mut conn, product.id, &body).await?;
    commit(&mut conn).await?;
    // The session keeps its objects after commit (expire_on_commit=False): the row as built, not reloaded.
    Ok(resp::json_status(
        201,
        price_row_json(
            id,
            product.id,
            &code,
            &body.amount,
            &body.valid_from,
            body.valid_to.as_ref(),
        ),
    ))
}

/// `PATCH /catalog/prices/{price_id}`.
pub async fn update_price_row(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(price_id): Path<String>,
    PyJson(body): PyJson<WithSet<PriceRowPatch>>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let row = writes::load_price_row(&mut conn, &price_id).await?;
    let r = writes::update_price_row(&mut conn, &row, &body.model, &body.set).await?;
    commit(&mut conn).await?;
    Ok(resp::json(price_row_json(
        r.id,
        r.product_id,
        &r.currency,
        &r.amount,
        &r.valid_from,
        r.valid_to.as_ref(),
    )))
}

/// `DELETE /catalog/prices/{price_id}` (204).
pub async fn delete_price_row(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(price_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let row = writes::load_price_row(&mut conn, &price_id).await?;
    writes::delete_price_row(&mut conn, row.id).await?;
    commit(&mut conn).await?;
    Ok(resp::no_content())
}

/// `PUT /catalog/{product_id}/tiers`: replace the SKU's volume tiers. Ranges are inclusive and may not
/// overlap.
pub async fn replace_tiers(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
    PyJson(body): PyJson<TiersBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_catalog_manager(&mut conn, &user).await?;
    let product = load_product(&mut conn, &product_id).await?;
    writes::replace_tiers(&mut conn, product.id, &body.tiers).await?;
    commit(&mut conn).await?;
    let rows = writes::tiers_of(&mut conn, product.id).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|t| {
            json!({"id": t.id.to_string(), "min_qty": dec_float(&t.min_qty),
                   "max_qty": dec_float_opt(t.max_qty.as_ref()), "discount_pct": dec_float(&t.discount_pct)})
        })
        .collect();
    Ok(resp::json(json!({ "tiers": out })))
}

// ─── Pricing preview ──────────────────────────────────────────────────────────────

/// `_require_pricing_access(current_user)`.
fn require_pricing_access(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin
        || [
            "sales:view_catalog",
            "sales:create_quotation",
            "finance:create_invoice",
        ]
        .iter()
        .any(|c| user.perms.contains(*c))
    {
        Ok(())
    } else {
        Err(ApiError::http(
            403,
            "Permission required: sales:view_catalog",
        ))
    }
}

granary_core::py_model! {
    /// `class PreviewBody(BaseModel)`.
    pub struct PreviewBody {
        pub currency: String,
        pub issue_date: Option<NaiveDate> = None,
        pub customer_org_id: Option<String> = None,
        /// header discount, as on the quotation
        pub discount_type: Option<String> = None,
        pub discount_value: Decimal = Decimal::ZERO => v::decimal_c(DecC::new().ge("0").le(MONEY_MAX)),
        pub lines: Vec<Map<String, Value>> = Vec::new(),
    }
}

/// `POST /pricing/preview`: price lines exactly as saving a quotation would, plus the suggested payment
/// terms. A typed price or discount is checked against the user's permissions.
pub async fn pricing_preview(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PreviewBody>,
) -> ApiResult<Response> {
    require_pricing_access(&user)?;
    let mut conn = db.conn().await?;
    let on_date = match body.issue_date {
        Some(d) => d,
        None => today_local(&mut conn).await?,
    };
    let mut lines = Vec::with_capacity(body.lines.len());
    for (i, d) in body.lines.iter().enumerate() {
        lines.push(pricing::line_in_from_dict(&Value::Object(d.clone()), i)?);
    }
    let org = body.customer_org_id.as_deref().filter(|s| !s.is_empty());
    let res = pricing::price_lines(
        &mut conn,
        &body.currency,
        on_date,
        org,
        &lines,
        Some(&user),
        true,
        true,
        None::<&HashSet<String>>,
        body.discount_type.as_deref(),
        Some(body.discount_value),
    )
    .await?;
    let mut out = pricing::result_json(&res);
    let terms = pricing::suggest_payment_terms(&mut conn, &res).await?;
    out["payment_terms"] = terms;
    Ok(resp::json(out))
}
