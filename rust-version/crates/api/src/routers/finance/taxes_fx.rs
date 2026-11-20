//! finance.py: taxes and exchange rates.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::validation::{v, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{ExchangeRate, Tax};

use super::ledger::multiple_rows;
use super::{f, fromiso, now_utc, require_any};
use crate::resp;

// ═══ Taxes ═══════════════════════════════════════════════════════════════════

/// `_tax_dict(t)`.
fn tax_json(t: &Tax) -> Value {
    json!({
        "id": t.id.to_string(), "code": t.code, "name": t.name,
        "law_reference": t.law_reference,
        "rate": f(&t.rate), "rate_base": t.rate_base,
        "tax_type": t.tax_type, "is_active": t.is_active,
        "applies_to_sales": t.applies_to_sales,
        "applies_to_purchases": t.applies_to_purchases,
        "notes": t.notes,
        "tax_payable_account_id": t.tax_payable_account_id.map(|u| u.to_string()),
        "tax_receivable_account_id": t.tax_receivable_account_id.map(|u| u.to_string()),
    })
}

fn rate_c(
) -> impl Fn(&granary_core::validation::PyObj, &mut granary_core::validation::Cx) -> Option<Decimal>
{
    v::decimal_c(
        v::DecC::new()
            .ge("0")
            .le("100")
            .max_digits(8)
            .decimal_places(4),
    )
}

granary_core::py_model! {
    /// `class TaxCreate(BaseModel)`.
    pub struct TaxCreate {
        pub code: String,
        pub name: String,
        pub law_reference: Option<String> = None,
        pub rate: Decimal => rate_c(),
        pub rate_base: String = "subtotal",
        pub tax_type: String,
        pub applies_to_sales: bool = true,
        pub applies_to_purchases: bool = false,
        pub tax_payable_account_id: Option<String> = None,
        pub tax_receivable_account_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class TaxUpdate(BaseModel)`.
    pub struct TaxUpdate {
        pub name: Option<String> = None,
        pub law_reference: Option<String> = None,
        pub rate: Option<Decimal> = None => v::nullable(rate_c()),
        pub rate_base: Option<String> = None,
        pub tax_type: Option<String> = None,
        pub is_active: Option<bool> = None,
        pub applies_to_sales: Option<bool> = None,
        pub applies_to_purchases: Option<bool> = None,
        pub tax_payable_account_id: Option<String> = None,
        pub tax_receivable_account_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct ActiveOnlyQuery {
        pub active_only: bool = false,
    }
}

/// `GET /taxes` (any signed-in user).
pub async fn list_taxes(
    CurrentUser(_user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ActiveOnlyQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let sql = if q.active_only {
        "SELECT * FROM taxes WHERE is_deleted = false AND is_active = true ORDER BY code"
    } else {
        "SELECT * FROM taxes WHERE is_deleted = false ORDER BY code"
    };
    let rows: Vec<Tax> = sqlx::query_as(sql).fetch_all(&mut *conn).await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(tax_json).collect(),
    )))
}

/// `POST /taxes` (201).
pub async fn create_tax(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<TaxCreate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_tax_config", "system:manage_tax"])?;
    let mut conn = db.conn().await?;
    let found: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM taxes WHERE code = $1 LIMIT 2")
        .bind(&body.code)
        .fetch_all(&mut *conn)
        .await?;
    if found.len() > 1 {
        return Err(multiple_rows());
    }
    if !found.is_empty() {
        return Err(ApiError::http(
            400,
            format!("Tax code {} already exists", body.code),
        ));
    }
    let tax: Tax = sqlx::query_as(
        "INSERT INTO taxes (id, code, name, law_reference, rate, rate_base, tax_type, applies_to_sales, \
         applies_to_purchases, tax_payable_account_id, tax_receivable_account_id, notes, is_active, \
         created_by_id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, true, $13, false) \
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&body.code)
    .bind(&body.name)
    .bind(&body.law_reference)
    .bind(body.rate)
    .bind(&body.rate_base)
    .bind(&body.tax_type)
    .bind(body.applies_to_sales)
    .bind(body.applies_to_purchases)
    .bind(pg_uuid_opt(body.tax_payable_account_id.as_deref())?)
    .bind(pg_uuid_opt(body.tax_receivable_account_id.as_deref())?)
    .bind(&body.notes)
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, tax_json(&tax)))
}

async fn load_tax(conn: &mut sqlx::PgConnection, tax_id: &str) -> ApiResult<Tax> {
    let tax: Option<Tax> =
        sqlx::query_as("SELECT * FROM taxes WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(tax_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    tax.ok_or_else(|| ApiError::not_found("Tax", Some(tax_id)))
}

/// `PATCH /taxes/{tax_id}`.
pub async fn update_tax(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(tax_id): Path<String>,
    PyJson(body): PyJson<TaxUpdate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_tax_config", "system:manage_tax"])?;
    let mut conn = db.conn().await?;
    let mut tax = load_tax(&mut conn, &tax_id).await?;
    // model_dump(exclude_none=True)
    if let Some(v) = body.name {
        tax.name = v;
    }
    if body.law_reference.is_some() {
        tax.law_reference = body.law_reference;
    }
    if let Some(v) = body.rate {
        tax.rate = v;
    }
    if body.rate_base.is_some() {
        tax.rate_base = body.rate_base;
    }
    if let Some(v) = body.tax_type {
        tax.tax_type = v;
    }
    if body.is_active.is_some() {
        tax.is_active = body.is_active;
    }
    if body.applies_to_sales.is_some() {
        tax.applies_to_sales = body.applies_to_sales;
    }
    if body.applies_to_purchases.is_some() {
        tax.applies_to_purchases = body.applies_to_purchases;
    }
    if let Some(v) = body.tax_payable_account_id.as_deref() {
        tax.tax_payable_account_id = Some(pg_uuid(v)?);
    }
    if let Some(v) = body.tax_receivable_account_id.as_deref() {
        tax.tax_receivable_account_id = Some(pg_uuid(v)?);
    }
    if body.notes.is_some() {
        tax.notes = body.notes;
    }
    let tax: Tax = sqlx::query_as(
        "UPDATE taxes SET name = $2, law_reference = $3, rate = $4, rate_base = $5, tax_type = $6, \
         is_active = $7, applies_to_sales = $8, applies_to_purchases = $9, tax_payable_account_id = $10, \
         tax_receivable_account_id = $11, notes = $12, updated_by_id = $13, updated_at = now() \
         WHERE id = $1 RETURNING *",
    )
    .bind(tax.id)
    .bind(&tax.name)
    .bind(&tax.law_reference)
    .bind(tax.rate)
    .bind(&tax.rate_base)
    .bind(&tax.tax_type)
    .bind(tax.is_active)
    .bind(tax.applies_to_sales)
    .bind(tax.applies_to_purchases)
    .bind(tax.tax_payable_account_id)
    .bind(tax.tax_receivable_account_id)
    .bind(&tax.notes)
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(tax_json(&tax)))
}

/// `DELETE /taxes/{tax_id}` (204).
pub async fn delete_tax(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(tax_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_tax_config"])?;
    let mut conn = db.conn().await?;
    let tax = load_tax(&mut conn, &tax_id).await?;
    sqlx::query(
        "UPDATE taxes SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(tax.id)
    .bind(now_utc())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ═══ Exchange Rates ══════════════════════════════════════════════════════════

granary_core::py_model! {
    /// `class FXRateCreate(BaseModel)`.
    pub struct FxRateCreate {
        pub from_currency: String,
        pub to_currency: String,
        pub rate: Decimal => v::decimal_c(v::DecC::new().gt("0").max_digits(24).decimal_places(12)),
        pub effective_date: String,
    }
}

granary_core::py_model! {
    pub struct ListRatesQuery {
        pub from_currency: Option<String> = None,
        pub to_currency: Option<String> = None,
    }
}

/// `GET /exchange-rates` (any signed-in user).
pub async fn list_exchange_rates(
    CurrentUser(_user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListRatesQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let from = q
        .from_currency
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase());
    let to = q
        .to_currency
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase());
    let mut sql = String::from("SELECT * FROM exchange_rates WHERE is_deleted = false");
    let mut binds = Vec::new();
    if let Some(c) = from {
        binds.push(c);
        sql.push_str(&format!(" AND from_currency = ${}", binds.len()));
    }
    if let Some(c) = to {
        binds.push(c);
        sql.push_str(&format!(" AND to_currency = ${}", binds.len()));
    }
    sql.push_str(" ORDER BY effective_date DESC");
    let mut query = sqlx::query_as::<_, ExchangeRate>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rates = query.fetch_all(&mut *conn).await?;
    Ok(resp::json(Value::Array(
        rates
            .iter()
            .map(|r| {
                json!({
                    "id": r.id.to_string(),
                    "from_currency": r.from_currency, "to_currency": r.to_currency,
                    "rate": f(&r.rate),
                    "effective_date": resp::date_iso(&r.effective_date),
                    "source": r.source,
                })
            })
            .collect(),
    )))
}

/// `POST /exchange-rates` (201): upsert for the pair and date.
pub async fn create_exchange_rate(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<FxRateCreate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_fx"])?;
    let eff_date = fromiso(&body.effective_date)?;
    let from_cur = body.from_currency.to_uppercase();
    let to_cur = body.to_currency.to_uppercase();
    let mut conn = db.conn().await?;
    let existing: Vec<ExchangeRate> = sqlx::query_as(
        "SELECT * FROM exchange_rates WHERE from_currency = $1 AND to_currency = $2 AND effective_date = $3 \
         AND is_deleted = false LIMIT 2",
    )
    .bind(&from_cur)
    .bind(&to_cur)
    .bind(eff_date)
    .fetch_all(&mut *conn)
    .await?;
    if existing.len() > 1 {
        return Err(multiple_rows());
    }
    if let Some(e) = existing.first() {
        let rate: Decimal = sqlx::query_scalar(
            "UPDATE exchange_rates SET rate = $2, updated_at = now() WHERE id = $1 RETURNING rate",
        )
        .bind(e.id)
        .bind(body.rate)
        .fetch_one(&mut *conn)
        .await?;
        drop(conn);
        db.commit().await?;
        return Ok(resp::json_status(
            201,
            json!({"id": e.id.to_string(), "rate": f(&rate), "updated": true}),
        ));
    }
    let fx: ExchangeRate = sqlx::query_as(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, source, \
         created_by_id, is_deleted) VALUES ($1, $2, $3, $4, $5, 'manual', $6, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&from_cur)
    .bind(&to_cur)
    .bind(body.rate)
    .bind(eff_date)
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "id": fx.id.to_string(),
            "from_currency": fx.from_currency, "to_currency": fx.to_currency,
            "rate": f(&fx.rate),
            "effective_date": resp::date_iso(&fx.effective_date),
        }),
    ))
}

granary_core::py_model! {
    pub struct SpotRateQuery {
        pub from_currency: String = "USD",
        pub to_currency: String = "USD",
        pub on_date: Option<String> = None,
    }
}

/// `GET /fx/rate`: the most recent rate for a pair on or before on_date.
pub async fn get_spot_rate(
    CurrentUser(_user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<SpotRateQuery>,
) -> ApiResult<Response> {
    let from_cur = q.from_currency.to_uppercase();
    let to_cur = q.to_currency.to_uppercase();
    let mut conn = db.conn().await?;
    let lookup = match q.on_date.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => fromiso(s)?,
        None => today_local(&mut conn).await?,
    };
    let rate =
        match granary_services::fx::get_rate(&mut conn, &from_cur, &to_cur, Some(lookup)).await {
            Ok(r) => r,
            Err(granary_services::fx::RateError::NoRate(_)) => {
                return Err(ApiError::http(
                    404,
                    format!("No rate found for {from_cur}/{to_cur}"),
                ))
            }
            Err(granary_services::fx::RateError::Other(e)) => return Err(e),
        };
    let row: Option<ExchangeRate> = sqlx::query_as(
        "SELECT * FROM exchange_rates WHERE from_currency = $1 AND to_currency = $2 AND is_deleted = false \
         AND effective_date <= $3 ORDER BY effective_date DESC LIMIT 1",
    )
    .bind(&from_cur)
    .bind(&to_cur)
    .bind(lookup)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(resp::json(json!({
        "from_currency": from_cur,
        "to_currency": to_cur,
        "rate": f(&rate),
        "effective_date": resp::date_iso(&row.as_ref().map(|r| r.effective_date).unwrap_or(lookup)),
        "source": match &row { Some(r) => json!(r.source), None => json!("computed") },
    })))
}

/// `DELETE /exchange-rates/{rate_id}` (204).
pub async fn delete_exchange_rate(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rate_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_fx"])?;
    let mut conn = db.conn().await?;
    let fx: Option<ExchangeRate> =
        sqlx::query_as("SELECT * FROM exchange_rates WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(&rate_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(fx) = fx else {
        return Err(ApiError::not_found("ExchangeRate", Some(&rate_id)));
    };
    sqlx::query(
        "UPDATE exchange_rates SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(fx.id)
    .bind(now_utc())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
