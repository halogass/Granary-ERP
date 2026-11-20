//! sales.py "Contracts" section.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::v::{self, DecC};
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::sales::Contract;
use granary_services::approvals::engine;

use super::{commit, db_str, fromisoformat_or_422, has, perm_403, truthy, MONEY_MAX};
use crate::resp::{self, date_iso_opt, iso_opt};

granary_core::py_model! {
    /// `class ContractCreate(BaseModel)`.
    pub struct ContractCreate {
        pub quotation_id: Option<String> = None,
        pub deal_id: Option<String> = None,
        pub customer_org_id: Option<String> = None,
        pub customer_contact_id: Option<String> = None,
        pub contract_type: String = "sales",
        pub value: Option<Decimal> = None => v::nullable(v::decimal_c(DecC::new().ge("0").le(MONEY_MAX))),
        pub currency: String = "USD",
        pub start_date: Option<String> = None,
        pub end_date: Option<String> = None,
        pub auto_renew: bool = false,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// The query parameters of `GET /contracts`.
    pub struct ContractListQuery {
        pub status: Option<String> = None,
        pub contract_type: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `sales:view_contracts` or `sales:create_contract` (or superadmin).
fn require_view(user: &AuthUser) -> ApiResult<()> {
    if has(user, "sales:view_contracts") || has(user, "sales:create_contract") {
        Ok(())
    } else {
        Err(perm_403("sales:view_contracts"))
    }
}

/// A live contract by the path string.
async fn load_contract(db: &mut PgConnection, contract_id: &str) -> ApiResult<Contract> {
    let id = pg_uuid(contract_id)?;
    let c: Option<Contract> = sqlx::query_as(
        "SELECT * FROM contracts WHERE contracts.id = $1 AND contracts.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    c.ok_or_else(|| ApiError::not_found("Contract", Some(contract_id)))
}

/// `GET /contracts`.
pub async fn list_contracts(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(p): PyQuery<ContractListQuery>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM contracts WHERE contracts.is_deleted = false");
    let mut binds: Vec<&str> = Vec::new();
    if let Some(s) = p.status.as_deref().filter(|s| !s.is_empty()) {
        binds.push(s);
        sql.push_str(&format!(" AND contracts.status = ${}", binds.len()));
    }
    if let Some(t) = p.contract_type.as_deref().filter(|s| !s.is_empty()) {
        binds.push(t);
        sql.push_str(&format!(" AND contracts.contract_type = ${}", binds.len()));
    }
    sql.push_str(&format!(
        " ORDER BY contracts.created_at DESC LIMIT ${} OFFSET ${}",
        binds.len() + 1,
        binds.len() + 2
    ));
    let mut q = sqlx::query_as::<_, Contract>(&sql);
    for b in &binds {
        q = q.bind(*b);
    }
    let rows = q.bind(p.limit).bind(p.offset).fetch_all(&mut *conn).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(), "number": c.number, "status": c.status,
                "contract_type": c.contract_type,
                "value": super::float_if(c.value), "currency": c.currency,
                "start_date": date_iso_opt(c.start_date.as_ref()),
                "end_date": date_iso_opt(c.end_date.as_ref()),
                "auto_renew": c.auto_renew,
                "customer_org_id": c.customer_org_id.map(|u| u.to_string()),
                "signed_at": iso_opt(c.signed_at.as_ref()),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `date.fromisoformat(data[f])` when truthy; an empty string stays a str and fails in asyncpg.
fn contract_date(s: &Option<String>) -> ApiResult<Option<chrono::NaiveDate>> {
    match s.as_deref() {
        None => Ok(None),
        Some("") => Err(ApiError::internal(anyhow::anyhow!(
            "invalid input for query argument: '' (expected a datetime.date or datetime.datetime instance, got 'str')"
        ))),
        Some(x) => fromisoformat_or_422(x).map(Some),
    }
}

/// `create_contract(body, current_user, db)`: the route and what nary/deals.py calls. Commits.
pub(crate) async fn create_contract_core(
    db: &mut PgConnection,
    body: &ContractCreate,
    user: &AuthUser,
) -> ApiResult<Value> {
    if !has(user, "sales:create_contract") {
        return Err(perm_403("sales:create_contract"));
    }
    let number = granary_services::numbering::next_document_number(db, "contract").await?;
    let start = contract_date(&body.start_date)?;
    let end = contract_date(&body.end_date)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contracts (id, number, quotation_id, deal_id, customer_org_id, customer_contact_id, status, \
         contract_type, value, currency, start_date, end_date, auto_renew, notes, created_by_id, is_deleted) \
         VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6::uuid, 'draft', $7, $8, $9, $10, $11, $12, $13, $14, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(&body.quotation_id)
    .bind(&body.deal_id)
    .bind(&body.customer_org_id)
    .bind(&body.customer_contact_id)
    .bind(&body.contract_type)
    .bind(body.value)
    .bind(&body.currency)
    .bind(start)
    .bind(end)
    .bind(body.auto_renew)
    .bind(&body.notes)
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    Ok(json!({"id": id.to_string(), "number": number, "status": "draft"}))
}

/// `create_contract(ContractCreate(...), actor, db)` as nary/deals.py calls it: Pydantic refuses a None
/// currency or an out-of-range value with a ValidationError (raised as is, not a 422).
pub(crate) async fn create_contract_hook(
    body: granary_services::later::nary_deals::ContractCreate,
    actor: &AuthUser,
    db: &mut PgConnection,
) -> ApiResult<Value> {
    let Some(currency) = body.currency.clone() else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "1 validation error for ContractCreate\ncurrency\n  Input should be a valid string"
        )));
    };
    if let Some(v) = body.value {
        let max: Decimal = MONEY_MAX.parse().expect("MONEY_MAX");
        if v < Decimal::ZERO || v > max {
            return Err(ApiError::internal(anyhow::anyhow!(
                "1 validation error for ContractCreate\nvalue\n  Input should be {} {}",
                if v < Decimal::ZERO {
                    "greater than or equal to"
                } else {
                    "less than or equal to"
                },
                if v < Decimal::ZERO { "0" } else { MONEY_MAX }
            )));
        }
    }
    let model = ContractCreate {
        quotation_id: body.quotation_id,
        deal_id: body.deal_id,
        customer_org_id: body.customer_org_id,
        customer_contact_id: body.customer_contact_id,
        contract_type: "sales".into(),
        value: body.value,
        currency,
        start_date: None,
        end_date: None,
        auto_renew: false,
        notes: body.notes,
    };
    create_contract_core(db, &model, actor).await
}

/// `POST /contracts` (201).
pub async fn create_contract(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ContractCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let out = create_contract_core(&mut conn, &body, &user).await?;
    Ok(resp::json_status(201, out))
}

/// `GET /contracts/{contract_id}`.
pub async fn get_contract(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(contract_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let c = load_contract(&mut conn, &contract_id).await?;
    Ok(resp::json(json!({
        "id": c.id.to_string(), "number": c.number, "status": c.status,
        "contract_type": c.contract_type,
        "value": super::float_if(c.value), "currency": c.currency,
        "start_date": date_iso_opt(c.start_date.as_ref()),
        "end_date": date_iso_opt(c.end_date.as_ref()),
        "auto_renew": c.auto_renew,
        "customer_org_id": c.customer_org_id.map(|u| u.to_string()),
        "customer_contact_id": c.customer_contact_id.map(|u| u.to_string()),
        "quotation_id": c.quotation_id.map(|u| u.to_string()),
        "deal_id": c.deal_id.map(|u| u.to_string()),
        "signed_at": iso_opt(c.signed_at.as_ref()),
        "signed_by_customer_name": c.signed_by_customer_name,
        "signed_by_customer_title": c.signed_by_customer_title,
        "signed_by_company_name": c.signed_by_company_name,
        "document_url": c.document_url,
        "notes": c.notes,
    })))
}

/// `PATCH /contracts/{contract_id}/status`.
pub async fn update_contract_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(contract_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    if !has(&user, "sales:create_contract") {
        return Err(perm_403("sales:create_contract"));
    }
    let mut conn = db.conn().await?;
    let mut c = load_contract(&mut conn, &contract_id).await?;
    let new_status = body.get("status").cloned().unwrap_or(Value::Null);
    let s = new_status.as_str();
    if matches!(s, Some("sent_for_signing" | "signed")) {
        // The Owner reviews every contract before it goes to the customer
        engine::ensure_or_submit(
            &mut conn,
            "sales_contract",
            c.id,
            &user,
            "sent for signing",
            None,
        )
        .await?;
    }
    let field = |k: &str| body.get(k).filter(|v| truthy(v));
    // Values go to VARCHAR columns: a non-str fails in asyncpg when the row is written (a 500).
    let status_db = db_str(&new_status)?;
    c.status = status_db.clone();
    if s == Some("signed") {
        c.signed_at = Some(Utc::now());
        granary_services::nary::events::emit(
            &mut conn,
            "contract.signed",
            Some("contract"),
            Some(c.id),
            Some(json!({
                "deal_id": c.deal_id.map(|u| u.to_string()),
                "quotation_id": c.quotation_id.map(|u| u.to_string()),
            })),
            Some(user.id),
        )
        .await?;
        if let Some(v) = field("signed_by_customer_name") {
            c.signed_by_customer_name = db_str(v)?;
        }
        if let Some(v) = field("signed_by_customer_title") {
            c.signed_by_customer_title = db_str(v)?;
        }
        if let Some(v) = field("signed_by_company_name") {
            c.signed_by_company_name = db_str(v)?;
        }
    }
    if let Some(v) = field("document_url") {
        c.document_url = db_str(v)?;
    }
    sqlx::query(
        "UPDATE contracts SET status = $2, signed_at = $3, signed_by_customer_name = $4, \
         signed_by_customer_title = $5, signed_by_company_name = $6, document_url = $7, \
         updated_at = CASE WHEN (status, signed_at, signed_by_customer_name, signed_by_customer_title, \
         signed_by_company_name, document_url) IS DISTINCT FROM ($2::varchar, $3::timestamptz, $4::varchar, \
         $5::varchar, $6::varchar, $7::varchar) THEN now() ELSE updated_at END WHERE id = $1",
    )
    .bind(c.id)
    .bind(&c.status)
    .bind(c.signed_at)
    .bind(&c.signed_by_customer_name)
    .bind(&c.signed_by_customer_title)
    .bind(&c.signed_by_company_name)
    .bind(&c.document_url)
    .execute(&mut *conn)
    .await?;
    commit(&mut conn).await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json(
        json!({"id": c.id.to_string(), "status": status_db}),
    ))
}

/// `DELETE /contracts/{contract_id}` (204): a soft delete of a draft or terminated contract.
pub async fn delete_contract(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(contract_id): Path<String>,
) -> ApiResult<Response> {
    if !has(&user, "sales:create_contract") {
        return Err(perm_403("sales:create_contract"));
    }
    let mut conn = db.conn().await?;
    let c = load_contract(&mut conn, &contract_id).await?;
    if !matches!(c.status.as_deref(), Some("draft" | "terminated")) {
        return Err(ApiError::http(
            400,
            "Only draft or terminated contracts can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE contracts SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(c.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    commit(&mut conn).await?;
    Ok(resp::no_content())
}
