//! backend/app/routers/crm.py, mounted at `/api/v1/crm` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! The PATCH routes take `body: dict` and `setattr` every allowed key onto the ORM row; the flush then
//! binds each value to its column through asyncpg. [`col_val`] reproduces what asyncpg accepts per column
//! type: a value it refuses (a number for a text column, a malformed UUID string) is a DBAPIError, which the
//! last-resort handler answers with a 500; NOT NULL / FK violations are the IntegrityError 409.
//! Path ids are plain `str` compared with UUID columns: a malformed id is the central 422 (asyncpg refuses it).

use crate::extract::pg_uuid;
use std::collections::HashSet;

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_compat::text::utils::escape_like;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm::{Activity, Contact, Deal, Lead, Organization};
use granary_services::crm::attributes::{self, py};
use granary_services::crm::stages::{self, StageFail};

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/crm";

/// The routes of `router` in crm.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/organizations"),
            get(list_orgs).post(create_org),
        )
        .route(
            &at(PREFIX, "/organizations/{org_id}"),
            get(get_org).patch(update_org).delete(delete_org),
        )
        .route(
            &at(PREFIX, "/organizations/{org_id}/enrich"),
            post(enrich_org),
        )
        .route(
            &at(PREFIX, "/contacts"),
            get(list_contacts).post(create_contact),
        )
        .route(
            &at(PREFIX, "/contacts/{contact_id}"),
            get(get_contact)
                .patch(update_contact)
                .delete(delete_contact),
        )
        .route(&at(PREFIX, "/stages"), get(get_funnel_stages))
        .route(&at(PREFIX, "/leads"), get(list_leads).post(create_lead))
        .route(
            &at(PREFIX, "/leads/{lead_id}"),
            get(get_lead).patch(update_lead).delete(delete_lead),
        )
        .route(
            &at(PREFIX, "/leads/{lead_id}/score"),
            post(score_lead_endpoint),
        )
        .route(&at(PREFIX, "/leads/{lead_id}/promote"), post(promote_lead))
        .route(&at(PREFIX, "/deals"), get(list_deals).post(create_deal))
        .route(
            &at(PREFIX, "/deals/{deal_id}"),
            get(get_deal).patch(update_deal).delete(delete_deal),
        )
        .route(
            &at(PREFIX, "/deals/{deal_id}/stage"),
            patch(update_deal_stage),
        )
        .route(
            &at(PREFIX, "/activities"),
            post(create_activity).get(list_activities),
        )
        .route(
            &at(PREFIX, "/activities-deleted"),
            get(list_voided_activities),
        )
        .route(
            &at(PREFIX, "/activities-deleted/{voided_id}/restore"),
            post(restore_voided_activity),
        )
        .route(
            &at(PREFIX, "/activities/{activity_id}"),
            get(get_activity)
                .patch(update_activity)
                .delete(delete_activity),
        )
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

const DEAL_DATE_FIELDS: [&str; 2] = ["expected_close_date", "actual_close_date"];

/// A JSON object with its keys in the given order (a long `json!` literal exceeds the macro recursion limit).
pub(crate) fn obj(items: Vec<(&str, Value)>) -> Value {
    Value::Object(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn has(user: &AuthUser, code: &str) -> bool {
    user.perms.contains(code)
}

fn can_view_crm(user: &AuthUser) -> bool {
    user.is_superadmin || has(user, "crm:view_all") || has(user, "crm:view_own")
}

fn need(user: &AuthUser, code: &str) -> ApiResult<()> {
    if !has(user, code) && !user.is_superadmin {
        return Err(ApiError::http(403, format!("Permission required: {code}")));
    }
    Ok(())
}

fn crm_view_required(user: &AuthUser) -> ApiResult<()> {
    if !can_view_crm(user) {
        return Err(ApiError::http(403, "CRM view permission required"));
    }
    Ok(())
}

fn internal(msg: String) -> ApiError {
    ApiError::internal(anyhow::anyhow!(msg))
}

fn s(v: &Option<String>) -> Value {
    v.clone().map(Value::String).unwrap_or(Value::Null)
}

fn uid(v: Option<Uuid>) -> Value {
    v.map(|u| Value::String(u.to_string()))
        .unwrap_or(Value::Null)
}

fn arr(v: &Option<Vec<String>>) -> Value {
    json!(v.clone().unwrap_or_default())
}

/// `x or {}` of a JSONB column.
fn or_empty(v: &Value) -> Value {
    if py::truthy(v) {
        v.clone()
    } else {
        json!({})
    }
}

/// `f"{first_name} {last_name or ''}".strip()`.
fn full_name(first: &str, last: Option<&str>) -> String {
    pystr::strip(&format!("{first} {}", last.unwrap_or(""))).to_string()
}

/// `_parse_date_field(value, field)`: None/"" clears, an ISO date sets, anything else is a 422.
fn parse_date_field(value: &Value, field: &str) -> ApiResult<Option<NaiveDate>> {
    match value {
        Value::Null => Ok(None),
        Value::String(t) if t.is_empty() => Ok(None),
        Value::String(t) => py::date_fromisoformat(t)
            .map(Some)
            .ok_or_else(|| ApiError::http(422, format!("{field} must be a date (YYYY-MM-DD)"))),
        _ => Err(ApiError::http(
            422,
            format!("{field} must be a date (YYYY-MM-DD)"),
        )),
    }
}

/// A column type as the flush binds it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    Uuid,
    Int,
    Num,
    Bool,
    Date,
    TextArr,
    Json,
}

/// A value ready to be bound.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SqlVal {
    Null,
    Text(String),
    Uuid(Uuid),
    Int(i32),
    Num(Decimal),
    Bool(bool),
    Date(NaiveDate),
    TextArr(Vec<String>),
    Json(Value),
}

pub(crate) fn py_type(v: &Value) -> &'static str {
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

fn refused(v: &Value, expected: &str) -> ApiError {
    internal(format!(
        "invalid input for query argument: {} (expected {expected}, got {})",
        py::str_of(v),
        py_type(v)
    ))
}

/// A Python float / int / str value as the Numeric column stores it.
fn num_of(v: &Value) -> Option<Decimal> {
    use std::str::FromStr;
    match v {
        Value::Number(n) => {
            let t = if n.is_f64() {
                granary_compat::pyjson::py_float_repr(n.as_f64()?)
            } else {
                n.to_string()
            };
            Decimal::from_str(&t)
                .ok()
                .or_else(|| Decimal::from_scientific(&t).ok())
        }
        Value::String(t) => Decimal::from_str(pystr::strip(t))
            .ok()
            .or_else(|| Decimal::from_scientific(pystr::strip(t)).ok()),
        _ => None,
    }
}

/// What the flush binds for `setattr(row, column, value)`; asyncpg refuses a mismatched type (a 500).
pub(crate) fn col_val(kind: Kind, v: &Value) -> ApiResult<SqlVal> {
    if v.is_null() && kind != Kind::Json {
        return Ok(SqlVal::Null);
    }
    match kind {
        Kind::Text => match v {
            Value::String(t) => Ok(SqlVal::Text(t.clone())),
            other => Err(refused(other, "str")),
        },
        Kind::Uuid => match v {
            Value::String(t) => pg_uuid(t).map(SqlVal::Uuid),
            other => Err(refused(other, "str or UUID")),
        },
        Kind::Int => match v {
            Value::Number(n) if n.is_i64() => n
                .as_i64()
                .and_then(|i| i32::try_from(i).ok())
                .map(SqlVal::Int)
                .ok_or_else(|| refused(v, "int")),
            Value::Bool(b) => Ok(SqlVal::Int(*b as i32)),
            other => Err(refused(other, "int")),
        },
        Kind::Num => num_of(v)
            .map(SqlVal::Num)
            .ok_or_else(|| refused(v, "Decimal")),
        Kind::Bool => match v {
            Value::Bool(b) => Ok(SqlVal::Bool(*b)),
            other => Err(refused(other, "bool")),
        },
        Kind::Date => match v {
            Value::String(t) => py::date_fromisoformat(t)
                .map(SqlVal::Date)
                .ok_or_else(|| refused(v, "datetime.date")),
            other => Err(refused(other, "datetime.date")),
        },
        Kind::TextArr => match v {
            Value::Array(items) => {
                let mut out = Vec::new();
                for it in items {
                    match it {
                        Value::String(t) => out.push(t.clone()),
                        other => return Err(refused(other, "str")),
                    }
                }
                Ok(SqlVal::TextArr(out))
            }
            other => Err(refused(other, "list")),
        },
        Kind::Json => Ok(SqlVal::Json(v.clone())),
    }
}

/// `UPDATE <table> SET ..., updated_at = now() WHERE id = <id>` for the changed columns (SQLAlchemy only
/// flushes attributes whose value changed; nothing changed: no UPDATE, `updated_at` untouched).
pub(crate) async fn write_columns(
    conn: &mut PgConnection,
    table: &str,
    id: Uuid,
    sets: &[(String, SqlVal)],
) -> ApiResult<()> {
    if sets.is_empty() {
        return Ok(());
    }
    let mut qb = QueryBuilder::<Postgres>::new(format!("UPDATE {table} SET "));
    for (i, (c, v)) in sets.iter().enumerate() {
        if i > 0 {
            qb.push(", ");
        }
        qb.push(format!("\"{c}\" = "));
        match v.clone() {
            SqlVal::Null => {
                qb.push("NULL");
            }
            SqlVal::Text(t) => {
                qb.push_bind(t);
            }
            SqlVal::Uuid(u) => {
                qb.push_bind(u);
            }
            SqlVal::Int(i) => {
                qb.push_bind(i);
            }
            SqlVal::Num(d) => {
                qb.push_bind(d);
            }
            SqlVal::Bool(b) => {
                qb.push_bind(b);
            }
            SqlVal::Date(d) => {
                qb.push_bind(d);
            }
            SqlVal::TextArr(a) => {
                qb.push_bind(a);
            }
            SqlVal::Json(j) => {
                qb.push_bind(j);
            }
        }
    }
    qb.push(", updated_at = now() WHERE id = ").push_bind(id);
    qb.build().execute(&mut *conn).await?;
    Ok(())
}

/// The row's current value of `column`, as `setattr` history compares it.
pub(crate) fn same_as_current(row: &Map<String, Value>, column: &str, new: &SqlVal) -> bool {
    let cur = row.get(column).cloned().unwrap_or(Value::Null);
    match new {
        SqlVal::Null => cur.is_null(),
        SqlVal::Text(t) => cur.as_str() == Some(t.as_str()),
        // A str never equals the stored UUID object.
        SqlVal::Uuid(_) => false,
        SqlVal::Int(i) => cur.as_i64() == Some(*i as i64),
        SqlVal::Num(d) => num_of(&cur).is_some_and(|c| c == *d),
        SqlVal::Bool(b) => cur.as_bool() == Some(*b),
        SqlVal::Date(d) => cur.as_str() == Some(d.to_string().as_str()),
        SqlVal::TextArr(a) => cur == json!(a),
        SqlVal::Json(j) => &cur == j,
    }
}

pub(crate) fn row_map<T: serde::Serialize>(row: &T) -> Map<String, Value> {
    match serde_json::to_value(row) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// `apply_custom` turning a bad value into a 400.
async fn set_custom(
    conn: &mut PgConnection,
    object_type: &str,
    custom: &mut Value,
    submitted: Option<&Value>,
) -> ApiResult<()> {
    attributes::apply_custom(conn, object_type, custom, submitted)
        .await
        .map_err(ApiError::from)
}

/// `_needs_next_action(db, entity_type, ids)`: the open records among `ids` with nothing planned.
async fn needs_next_action(
    conn: &mut PgConnection,
    entity_type: &str,
    ids: &[Uuid],
) -> ApiResult<HashSet<Uuid>> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let today = granary_core::clock::today_local(conn)
        .await
        .map_err(ApiError::from)?;
    let mut planned: HashSet<Uuid> = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT entity_id FROM crm_reminders WHERE entity_type = $1 AND entity_id = ANY($2) \
         AND status = 'open'",
    )
    .bind(entity_type)
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .flatten()
    .collect();
    let acts: Vec<Uuid> = sqlx::query_scalar(
        "SELECT entity_id FROM activities WHERE entity_type = $1 AND entity_id = ANY($2) \
         AND next_action_due >= $3 AND is_completed IS NOT true",
    )
    .bind(entity_type)
    .bind(ids)
    .bind(today)
    .fetch_all(&mut *conn)
    .await?;
    planned.extend(acts);
    Ok(ids
        .iter()
        .filter(|i| !planned.contains(i))
        .copied()
        .collect())
}

/// `_notify_assignment(db, assignee_id, by_user, kind, record)`: True when someone was told.
async fn notify_assignment(
    conn: &mut PgConnection,
    assignee: &Value,
    by_user: &AuthUser,
    kind: &str,
    record_id: Uuid,
    title: &Value,
) -> ApiResult<bool> {
    if !py::truthy(assignee) || py::str_of(assignee) == by_user.id.to_string() {
        return Ok(false);
    }
    let link = if kind == "lead" {
        format!("/crm/leads/{record_id}")
    } else {
        format!("/crm/deals/{record_id}")
    };
    let title_s = if py::truthy(title) {
        py::str_of(title)
    } else {
        "untitled".to_string()
    };
    let head = format!("{} assigned you a {kind}: {title_s}", by_user.name);
    let body = format!("Open the {kind} to follow up.");
    let rid = record_id.to_string();
    granary_services::approvals::notify::notify(
        conn,
        [py::str_of(assignee)],
        granary_services::approvals::notify::Notice::new(&head, &body, &link, kind, &rid)
            .type_("crm_assignment")
            .email(true),
    )
    .await?;
    Ok(true)
}

/// `_queue_enrichment(db, org, user)`: best effort, never an error.
async fn queue_enrichment(conn: &mut PgConnection, org_id: Uuid, user: &AuthUser) -> bool {
    match granary_services::ai::enrichment::settings(conn).await {
        Ok(cfg) if cfg.on_create => {}
        _ => return false,
    }
    granary_services::tasks::delay(
        "enrich_organization_task",
        vec![json!(org_id.to_string()), json!(user.id.to_string())],
        Map::new(),
    )
    .await
    .is_ok()
}

async fn load_org(conn: &mut PgConnection, org_id: &str) -> ApiResult<Organization> {
    let id = pg_uuid(org_id)?;
    sqlx::query_as::<_, Organization>(
        "SELECT * FROM organizations WHERE id = $1 AND is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("Organization", Some(org_id)))
}

async fn load_contact(conn: &mut PgConnection, contact_id: &str) -> ApiResult<Contact> {
    let id = pg_uuid(contact_id)?;
    sqlx::query_as::<_, Contact>("SELECT * FROM contacts WHERE id = $1 AND is_deleted = false")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("Contact", Some(contact_id)))
}

async fn load_lead(conn: &mut PgConnection, lead_id: &str) -> ApiResult<Lead> {
    let id = pg_uuid(lead_id)?;
    sqlx::query_as::<_, Lead>("SELECT * FROM leads WHERE id = $1 AND is_deleted = false")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("Lead", Some(lead_id)))
}

async fn load_deal(conn: &mut PgConnection, deal_id: &str) -> ApiResult<Deal> {
    let id = pg_uuid(deal_id)?;
    sqlx::query_as::<_, Deal>("SELECT * FROM deals WHERE id = $1 AND is_deleted = false")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("Deal", Some(deal_id)))
}

/// Applies the allowed keys of a PATCH body: (column, value) pairs that differ from the row.
fn collect_sets(
    body: &Map<String, Value>,
    allowed: &[(&str, Kind)],
    row: &Map<String, Value>,
) -> ApiResult<Vec<(String, SqlVal)>> {
    let mut sets: Vec<(String, SqlVal)> = Vec::new();
    for (k, v) in body {
        let Some((_, kind)) = allowed.iter().find(|(name, _)| name == k) else {
            continue;
        };
        let val = col_val(*kind, v)?;
        sets.retain(|(c, _)| c != k);
        if !same_as_current(row, k, &val) {
            sets.push((k.clone(), val));
        }
    }
    Ok(sets)
}

// ─── Organizations ───────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class OrgCreate(BaseModel)`.
    pub struct OrgCreate {
        pub name: String,
        pub industry: Option<String> = None,
        pub website: Option<String> = None,
        pub country: Option<String> = None,
        pub city: Option<String> = None,
        pub address: Option<String> = None,
        pub phone: Option<String> = None,
        pub email: Option<String> = None,
        pub tax_id: Option<String> = None,
        pub notes: Option<String> = None,
        pub custom: Option<Map<String, Value>> = None,
    }
}

granary_core::py_model! {
    pub struct ListOrgsQ {
        pub q: Option<String> = None,
        pub search: Option<String> = None,
        pub domain: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /organizations`.
pub async fn list_orgs(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListOrgsQ>,
) -> ApiResult<Response> {
    crm_view_required(&user)?;
    let mut conn = db.conn().await?;
    let mut qb =
        QueryBuilder::<Postgres>::new("SELECT * FROM organizations WHERE is_deleted = false");
    // 10 Nov 2026: the list page searches the name, website, email, city, country and industry (it used to send
    // `search`, which nothing read, so typing in the box did nothing).
    if let Some(text) =
        q.q.as_deref()
            .or(q.search.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty())
    {
        let pat = format!("%{}%", escape_like(text));
        qb.push(" AND (");
        for (i, col) in ["name", "website", "email", "city", "country", "industry"]
            .iter()
            .enumerate()
        {
            if i > 0 {
                qb.push(" OR ");
            }
            qb.push(format!("{col} ILIKE "))
                .push_bind(pat.clone())
                .push(" ESCAPE '\\'");
        }
        qb.push(")");
    }
    if let Some(domain) = q.domain.as_deref().filter(|t| !t.is_empty()) {
        let d = escape_like(pystr::strip(&pystr::lower(domain)));
        qb.push(" AND (lower(website) LIKE ")
            .push_bind(format!("%{d}%"))
            .push(" ESCAPE '\\' OR lower(email) LIKE ")
            .push_bind(format!("%@{d}"))
            .push(" ESCAPE '\\')");
    }
    qb.push(" ORDER BY name LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let orgs: Vec<Organization> = qb.build_query_as().fetch_all(&mut *conn).await?;
    let out: Vec<Value> = orgs
        .iter()
        .map(|o| {
            json!({
                "id": o.id.to_string(), "name": o.name, "industry": s(&o.industry),
                "website": s(&o.website), "country": s(&o.country), "city": s(&o.city),
                "email": s(&o.email), "phone": s(&o.phone), "address": s(&o.address),
                "tax_id": s(&o.tax_id), "notes": s(&o.notes),
                "created_at": resp::iso(&o.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /organizations` (201).
pub async fn create_org(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<OrgCreate>,
) -> ApiResult<Response> {
    need(&user, "crm:create_contact")?;
    let mut conn = db.conn().await?;
    let mut custom = Value::Null;
    let submitted = body.custom.clone().map(Value::Object);
    set_custom(&mut conn, "organization", &mut custom, submitted.as_ref()).await?;
    if custom.is_null() {
        custom = json!({}); // default=dict at flush
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (name, type, industry, country, city, address, website, phone, email, \
         tax_id, revenue_currency, notes, tags, created_by_id, custom, id, is_deleted, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, false, now(), now())",
    )
    .bind(&body.name)
    .bind(vec!["prospect".to_string()])
    .bind(&body.industry)
    .bind(&body.country)
    .bind(&body.city)
    .bind(&body.address)
    .bind(&body.website)
    .bind(&body.phone)
    .bind(&body.email)
    .bind(&body.tax_id)
    .bind("USD")
    .bind(&body.notes)
    .bind(Vec::<String>::new())
    .bind(user.id)
    .bind(&custom)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let enriching = queue_enrichment(&mut conn, id, &user).await;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name, "enriching": enriching}),
    ))
}

/// `GET /organizations/{org_id}`.
pub async fn get_org(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(org_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let org = load_org(&mut conn, &org_id).await?;
    granary_services::crm::access::assert_can_view(&user, org.assigned_to_id)?;
    Ok(resp::json(obj(vec![
        ("id", json!(org.id.to_string())),
        ("name", json!(org.name)),
        ("industry", json!(s(&org.industry))),
        ("website", json!(s(&org.website))),
        ("country", json!(s(&org.country))),
        ("city", json!(s(&org.city))),
        ("address", json!(s(&org.address))),
        ("phone", json!(s(&org.phone))),
        ("email", json!(s(&org.email))),
        ("tax_id", json!(s(&org.tax_id))),
        ("notes", json!(s(&org.notes))),
        ("description", json!(s(&org.description))),
        ("employee_count", json!(org.employee_count)),
        ("type", json!(arr(&org.type_))),
        ("tags", json!(arr(&org.tags))),
        ("assigned_to_id", json!(uid(org.assigned_to_id))),
        ("created_at", json!(resp::iso(&org.created_at))),
        ("custom", json!(or_empty(&org.custom))),
        // 2 Nov 2026: quoted without tax (migration 0147)
        ("quote_without_tax", json!(org.quote_without_tax)),
    ])))
}

/// `POST /organizations/{org_id}/enrich`: research the company now and fill what is still empty.
pub async fn enrich_org(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(org_id): Path<String>,
) -> ApiResult<Response> {
    use granary_services::ai::enrichment::{enrich_organization, EnrichError};
    need(&user, "crm:create_contact")?;
    let mut conn = db.conn().await?;
    let mut org = load_org(&mut conn, &org_id).await?;
    granary_services::crm::access::assert_can_view(&user, org.assigned_to_id)?;
    let out = match enrich_organization(&mut conn, &mut org, Some(user.id), None).await {
        Ok(out) => out,
        Err(EnrichError::Unavailable(e)) => return Err(ApiError::http(400, e.0)),
        Err(EnrichError::Other(e)) => return Err(e.into()),
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

const ORG_ALLOWED: [(&str, Kind); 13] = [
    ("name", Kind::Text),
    ("industry", Kind::Text),
    ("website", Kind::Text),
    ("country", Kind::Text),
    ("city", Kind::Text),
    ("address", Kind::Text),
    ("postal_code", Kind::Text),
    ("phone", Kind::Text),
    ("email", Kind::Text),
    ("tax_id", Kind::Text),
    ("notes", Kind::Text),
    ("description", Kind::Text),
    // 2 Nov 2026: quoted without tax (migration 0147)
    ("quote_without_tax", Kind::Bool),
];

/// `PATCH /organizations/{org_id}`.
pub async fn update_org(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(org_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "crm:edit_contact")?;
    let mut conn = db.conn().await?;
    let org = load_org(&mut conn, &org_id).await?;
    granary_services::crm::access::assert_can_view(&user, org.assigned_to_id)?;
    let row = row_map(&org);
    let mut sets = collect_sets(&body, &ORG_ALLOWED, &row)?;
    let mut custom = org.custom.clone();
    set_custom(&mut conn, "organization", &mut custom, body.get("custom")).await?;
    if custom != org.custom {
        sets.push(("custom".into(), SqlVal::Json(custom)));
    }
    write_columns(&mut conn, "organizations", org.id, &sets).await?;
    let name = sets
        .iter()
        .find(|(c, _)| c == "name")
        .map(|(_, v)| match v {
            SqlVal::Text(t) => Value::String(t.clone()),
            _ => Value::Null,
        })
        .unwrap_or_else(|| Value::String(org.name.clone()));
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": org.id.to_string(), "name": name})))
}

/// Soft delete of one row (`is_deleted = True; deleted_at = datetime.now(timezone.utc)`).
async fn soft_delete(conn: &mut PgConnection, table: &str, id: Uuid) -> ApiResult<()> {
    sqlx::query(&format!(
        "UPDATE {table} SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1"
    ))
    .bind(id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `DELETE /organizations/{org_id}` (204).
pub async fn delete_org(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(org_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "crm:delete_contact")?;
    let mut conn = db.conn().await?;
    let org = load_org(&mut conn, &org_id).await?;
    granary_services::crm::access::assert_can_view(&user, org.assigned_to_id)?;
    soft_delete(&mut conn, "organizations", org.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Contacts ────────────────────────────────────────────────────────────────

const CHAT_ID_MSG: &str =
    "Enter a phone number, not a WhatsApp chat id (the number is hidden by WhatsApp; \
                           leave it empty and link the chat instead)";

/// `_check_number(v)`: `if v and "@" in str(v)`.
fn bad_number(v: Option<&Value>) -> bool {
    match v {
        Some(v) if py::truthy(v) => py::str_of(v).contains('@'),
        _ => false,
    }
}

/// `phone` / `whatsapp` with the `_a_number` field validator ("Value error, ..." at the field).
fn number_field(v: &PyObj, cx: &mut Cx) -> Option<Option<String>> {
    let out = <Option<String> as Validate>::validate(v, cx)?;
    if out
        .as_deref()
        .is_some_and(|t| !t.is_empty() && t.contains('@'))
    {
        cx.err(format!("Value error, {CHAT_ID_MSG}"));
        return None;
    }
    Some(out)
}

granary_core::py_model! {
    /// `class ContactCreate(BaseModel)`.
    pub struct ContactCreate {
        pub first_name: String,
        pub last_name: Option<String> = None,
        pub email: Option<String> = None,
        pub phone: Option<String> = None => number_field,
        pub whatsapp: Option<String> = None => number_field,
        pub position: Option<String> = None,
        pub department: Option<String> = None,
        pub organization_id: Option<String> = None,
        pub country: Option<String> = None,
        pub linkedin_url: Option<String> = None,
        pub notes: Option<String> = None,
        pub custom: Option<Map<String, Value>> = None,
    }
}

granary_core::py_model! {
    pub struct ListContactsQ {
        pub q: Option<String> = None,
        pub org_id: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /contacts`.
pub async fn list_contacts(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListContactsQ>,
) -> ApiResult<Response> {
    crm_view_required(&user)?;
    let mut conn = db.conn().await?;
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM contacts WHERE is_deleted = false");
    if !(user.is_superadmin || has(&user, "crm:view_all")) {
        qb.push(" AND assigned_to_id = ").push_bind(user.id);
    }
    if let Some(text) = q.q.as_deref().filter(|t| !t.is_empty()) {
        let pat = format!("%{}%", escape_like(text));
        qb.push(" AND (first_name ILIKE ")
            .push_bind(pat.clone())
            .push(" ESCAPE '\\' OR last_name ILIKE ")
            .push_bind(pat.clone())
            .push(" ESCAPE '\\' OR email ILIKE ")
            .push_bind(pat)
            .push(" ESCAPE '\\')");
    }
    if let Some(org) = q.org_id.as_deref().filter(|t| !t.is_empty()) {
        qb.push(" AND organization_id = ").push_bind(pg_uuid(org)?);
    }
    // No ORDER BY in Python: the table's physical order.
    qb.push(" LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let rows: Vec<Contact> = qb.build_query_as().fetch_all(&mut *conn).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(), "first_name": c.first_name, "last_name": s(&c.last_name),
                "full_name": full_name(&c.first_name, c.last_name.as_deref()),
                "email": s(&c.email), "phone": s(&c.phone), "whatsapp": s(&c.whatsapp),
                "position": s(&c.position), "department": s(&c.department),
                "organization_id": uid(c.organization_id),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /contacts` (201).
pub async fn create_contact(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ContactCreate>,
) -> ApiResult<Response> {
    need(&user, "crm:create_contact")?;
    let mut conn = db.conn().await?;
    let org_id = match body.organization_id.as_deref() {
        Some(t) => Some(pg_uuid(t)?),
        None => None,
    };
    let mut custom = Value::Null;
    let submitted = body.custom.clone().map(Value::Object);
    set_custom(&mut conn, "contact", &mut custom, submitted.as_ref()).await?;
    if custom.is_null() {
        custom = json!({});
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (organization_id, first_name, last_name, email, phone, whatsapp, position, \
         department, is_primary, linkedin_url, country, notes, tags, assigned_to_id, created_by_id, custom, id, \
         is_deleted, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, $9, $10, $11, $12, $13, $14, $15, $16, false, now(), now())",
    )
    .bind(org_id)
    .bind(&body.first_name)
    .bind(&body.last_name)
    .bind(&body.email)
    .bind(&body.phone)
    .bind(&body.whatsapp)
    .bind(&body.position)
    .bind(&body.department)
    .bind(&body.linkedin_url)
    .bind(&body.country)
    .bind(&body.notes)
    .bind(Vec::<String>::new())
    .bind(user.id)
    .bind(user.id)
    .bind(&custom)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    let has_number = body.phone.as_deref().is_some_and(|t| !t.is_empty())
        || body.whatsapp.as_deref().is_some_and(|t| !t.is_empty());
    if has_number {
        // A chat with this number that came in earlier joins the contact.
        granary_services::nary::chat_links::link_by_number(&mut conn, id).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "first_name": body.first_name, "last_name": body.last_name}),
    ))
}

/// `GET /contacts/{contact_id}`.
pub async fn get_contact(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(contact_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let c = load_contact(&mut conn, &contact_id).await?;
    granary_services::crm::access::assert_can_view(&user, c.assigned_to_id)?;
    Ok(resp::json(obj(vec![
        ("id", json!(c.id.to_string())),
        ("first_name", json!(c.first_name)),
        ("last_name", json!(s(&c.last_name))),
        ("email", json!(s(&c.email))),
        ("phone", json!(s(&c.phone))),
        ("whatsapp", json!(s(&c.whatsapp))),
        ("position", json!(s(&c.position))),
        ("department", json!(s(&c.department))),
        ("organization_id", json!(uid(c.organization_id))),
        ("country", json!(s(&c.country))),
        ("linkedin_url", json!(s(&c.linkedin_url))),
        ("notes", json!(s(&c.notes))),
        ("is_primary", json!(c.is_primary)),
        ("tags", json!(arr(&c.tags))),
        ("assigned_to_id", json!(uid(c.assigned_to_id))),
        ("created_at", json!(resp::iso(&c.created_at))),
        ("custom", json!(or_empty(&c.custom))),
    ])))
}

const CONTACT_ALLOWED: [(&str, Kind); 12] = [
    ("first_name", Kind::Text),
    ("last_name", Kind::Text),
    ("email", Kind::Text),
    ("phone", Kind::Text),
    ("whatsapp", Kind::Text),
    ("position", Kind::Text),
    ("department", Kind::Text),
    ("notes", Kind::Text),
    ("organization_id", Kind::Uuid),
    ("linkedin_url", Kind::Text),
    ("country", Kind::Text),
    ("is_primary", Kind::Bool),
];

/// `PATCH /contacts/{contact_id}`.
pub async fn update_contact(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(contact_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "crm:edit_contact")?;
    let mut conn = db.conn().await?;
    let c = load_contact(&mut conn, &contact_id).await?;
    granary_services::crm::access::assert_can_view(&user, c.assigned_to_id)?;
    for k in ["phone", "whatsapp"] {
        if bad_number(body.get(k)) {
            return Err(ApiError::http(422, CHAT_ID_MSG));
        }
    }
    let row = row_map(&c);
    let mut sets = collect_sets(&body, &CONTACT_ALLOWED, &row)?;
    let mut custom = c.custom.clone();
    set_custom(&mut conn, "contact", &mut custom, body.get("custom")).await?;
    if custom != c.custom {
        sets.push(("custom".into(), SqlVal::Json(custom)));
    }
    write_columns(&mut conn, "contacts", c.id, &sets).await?;
    if body.contains_key("phone") || body.contains_key("whatsapp") {
        granary_services::nary::chat_links::link_by_number(&mut conn, c.id).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": c.id.to_string()})))
}

/// `DELETE /contacts/{contact_id}` (204).
pub async fn delete_contact(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(contact_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "crm:delete_contact")?;
    let mut conn = db.conn().await?;
    let c = load_contact(&mut conn, &contact_id).await?;
    granary_services::crm::access::assert_can_view(&user, c.assigned_to_id)?;
    soft_delete(&mut conn, "contacts", c.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Funnel vocabulary ───────────────────────────────────────────────────────

/// `GET /stages`: the one funnel vocabulary.
pub async fn get_funnel_stages(CurrentUser(_user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut reg = stages::registry(&mut conn).await.map_err(ApiError::from)?;
    // the allowed segments (crm.segments; empty: free text), read by the lead, deal and catalog forms
    reg["segments"] = json!(granary_services::segment::allowed(&mut conn).await?);
    Ok(resp::json(reg))
}

// ─── Leads ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class LeadCreate(BaseModel)`.
    pub struct LeadCreate {
        pub title: Option<String> = None,
        pub contact_id: Option<String> = None,
        pub organization_id: Option<String> = None,
        pub source: String = "founder_network",
        pub source_detail: Option<String> = None,
        pub status: String = "new",
        pub estimated_value: Option<Decimal> = None,
        pub currency: Option<String> = None,
        pub description: Option<String> = None,
        pub raw_message: Option<String> = None,
        pub priority: String = "medium",
        pub interested_products: Vec<String> = Vec::new(),
        pub source_campaign_id: Option<String> = None,
        pub attribution: Option<Map<String, Value>> = None,
        pub custom: Option<Map<String, Value>> = None,
        /// optional; a value of crm.segments when it lists any (services::segment)
        pub segment: Value = Value::Null,
    }
}

/// A submitted segment (services::segment), checked against crm.segments (422 naming the values); None clears it.
async fn segment_of(conn: &mut PgConnection, v: &Value) -> ApiResult<Option<String>> {
    granary_services::segment::check(conn, granary_services::segment::FIELD, v)
        .await?
        .map_err(|e| ApiError::http(422, e))
}

/// `_lead_out(l, needs_next)`: the funnel fields every lead response carries.
fn lead_out(l: &Lead, needs_next: &HashSet<Uuid>) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("interested_products".into(), arr(&l.interested_products));
    m.insert("source_campaign_id".into(), uid(l.source_campaign_id));
    m.insert(
        "attribution".into(),
        match &l.attribution {
            Some(a) if py::truthy(a) => a.clone(),
            _ => Value::Null,
        },
    );
    m.insert(
        "disqualification_reason".into(),
        s(&l.disqualification_reason),
    );
    m.insert("converted_to_deal_id".into(), uid(l.converted_to_deal_id));
    m.insert(
        "converted_at".into(),
        resp::iso_opt(l.converted_at.as_ref()),
    );
    m.insert(
        "needs_next_action".into(),
        Value::Bool(needs_next.contains(&l.id)),
    );
    m.insert("segment".into(), s(&l.segment));
    m
}

/// `float(x) if x else None` of a Numeric.
fn float_if(d: &Option<Decimal>) -> Value {
    match d {
        Some(d) if !d.is_zero() => resp::dec_float(d),
        _ => Value::Null,
    }
}

fn extend(mut m: Map<String, Value>, v: Value) -> Value {
    if let Value::Object(o) = v {
        for (k, x) in o {
            m.insert(k, x);
        }
    }
    Value::Object(m)
}

/// The contact and organization a lead / deal points at (selectinload: deleted rows included).
struct Links {
    contact: Option<Contact>,
    org_name: Option<String>,
}

async fn links(
    conn: &mut PgConnection,
    contact_id: Option<Uuid>,
    org_id: Option<Uuid>,
) -> ApiResult<Links> {
    let contact = match contact_id {
        Some(id) => {
            sqlx::query_as::<_, Contact>("SELECT * FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let org_name = match org_id {
        Some(id) => {
            sqlx::query_scalar::<_, String>("SELECT name FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    Ok(Links { contact, org_name })
}

fn contact_name(c: &Option<Contact>) -> Value {
    c.as_ref()
        .map(|c| Value::String(full_name(&c.first_name, c.last_name.as_deref())))
        .unwrap_or(Value::Null)
}

fn contact_field(c: &Option<Contact>, f: impl Fn(&Contact) -> &Option<String>) -> Value {
    c.as_ref().map(|c| s(f(c))).unwrap_or(Value::Null)
}

granary_core::py_model! {
    pub struct ListLeadsQ {
        pub status: Option<String> = None,
        /// a segment, or "none" for the leads without one
        pub segment: Option<String> = None,
        pub contact_id: Option<String> = None,
        pub organization_id: Option<String> = None,
        /// 10 Nov 2026: text search (title, organization, contact); `search` is read too
        pub q: Option<String> = None,
        pub search: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// 10 Nov 2026: `q` (or `search`) on the lead and deal lists: the title, the organization's name and the
/// contact's name or email. The pickers sent `search`, which nothing read, so they always showed the newest rows.
fn push_text_filter(qb: &mut QueryBuilder<'_, Postgres>, q: Option<&str>, search: Option<&str>) {
    let Some(text) = q.or(search).map(str::trim).filter(|t| !t.is_empty()) else {
        return;
    };
    let pat = format!("%{}%", escape_like(text));
    qb.push(" AND (title ILIKE ")
        .push_bind(pat.clone())
        .push(" ESCAPE '\\'");
    qb.push(" OR organization_id IN (SELECT id FROM organizations WHERE name ILIKE ")
        .push_bind(pat.clone())
        .push(" ESCAPE '\\')");
    qb.push(" OR contact_id IN (SELECT id FROM contacts WHERE first_name ILIKE ")
        .push_bind(pat.clone())
        .push(" ESCAPE '\\' OR last_name ILIKE ")
        .push_bind(pat.clone())
        .push(" ESCAPE '\\' OR email ILIKE ")
        .push_bind(pat)
        .push(" ESCAPE '\\'))");
}

/// `segment=<value>` (any case), or `none` for the rows without one.
fn push_segment_filter(qb: &mut QueryBuilder<'_, Postgres>, segment: Option<&str>) {
    let Some(seg) = segment.map(str::trim).filter(|t| !t.is_empty()) else {
        return;
    };
    if seg == "none" {
        qb.push(" AND segment IS NULL");
        return;
    }
    qb.push(" AND lower(segment) = lower(")
        .push_bind(seg.to_string())
        .push(")");
}

/// `GET /leads`.
pub async fn list_leads(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListLeadsQ>,
) -> ApiResult<Response> {
    crm_view_required(&user)?;
    let mut conn = db.conn().await?;
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM leads WHERE is_deleted = false");
    if !(user.is_superadmin || has(&user, "crm:view_all")) {
        qb.push(" AND assigned_to_id = ").push_bind(user.id);
    }
    if let Some(st) = q.status.as_deref().filter(|t| !t.is_empty()) {
        qb.push(" AND status = ").push_bind(st.to_string());
    }
    push_segment_filter(&mut qb, q.segment.as_deref());
    push_text_filter(&mut qb, q.q.as_deref(), q.search.as_deref());
    let cid = q.contact_id.as_deref().filter(|t| !t.is_empty());
    let oid = q.organization_id.as_deref().filter(|t| !t.is_empty());
    match (cid, oid) {
        (Some(c), Some(o)) => {
            qb.push(" AND (contact_id = ")
                .push_bind(pg_uuid(c)?)
                .push(" OR organization_id = ")
                .push_bind(pg_uuid(o)?)
                .push(")");
        }
        (Some(c), None) => {
            qb.push(" AND contact_id = ").push_bind(pg_uuid(c)?);
        }
        (None, Some(o)) => {
            qb.push(" AND organization_id = ").push_bind(pg_uuid(o)?);
        }
        (None, None) => {}
    }
    qb.push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let leads: Vec<Lead> = qb.build_query_as().fetch_all(&mut *conn).await?;
    let active: Vec<Uuid> = leads
        .iter()
        .filter(|l| {
            l.status
                .as_deref()
                .is_some_and(|st| stages::ACTIVE_LEAD_STATUSES.contains(&st))
        })
        .map(|l| l.id)
        .collect();
    let needs_next = needs_next_action(&mut conn, "lead", &active).await?;
    let mut out = Vec::new();
    for l in &leads {
        let lk = links(&mut conn, l.contact_id, l.organization_id).await?;
        out.push(extend(
            lead_out(l, &needs_next),
            obj(vec![
                ("id", json!(l.id.to_string())),
                ("title", json!(s(&l.title))),
                ("status", json!(s(&l.status))),
                ("priority", json!(s(&l.priority))),
                ("source", json!(l.source)),
                ("estimated_value", json!(float_if(&l.estimated_value))),
                ("currency", json!(s(&l.currency))),
                ("contact_id", json!(uid(l.contact_id))),
                ("organization_id", json!(uid(l.organization_id))),
                ("contact_name", json!(contact_name(&lk.contact))),
                (
                    "contact_email",
                    json!(contact_field(&lk.contact, |c| &c.email)),
                ),
                (
                    "contact_phone",
                    json!(contact_field(&lk.contact, |c| &c.phone)),
                ),
                (
                    "contact_whatsapp",
                    json!(contact_field(&lk.contact, |c| &c.whatsapp)),
                ),
                ("organization_name", json!(s(&lk.org_name))),
                ("description", json!(s(&l.description))),
                ("ai_score", json!(l.ai_score.clone().unwrap_or(Value::Null))),
                ("assigned_to_id", json!(uid(l.assigned_to_id))),
                ("created_by_id", json!(uid(l.created_by_id))),
                ("created_at", json!(resp::iso(&l.created_at))),
            ]),
        ));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /leads` (201).
pub async fn create_lead(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<LeadCreate>,
) -> ApiResult<Response> {
    need(&user, "crm:create_lead")?;
    let mut conn = db.conn().await?;
    let segment = segment_of(&mut conn, &body.segment).await?;
    stages::validate_lead_status(Some(&body.status), true).map_err(ApiError::from)?;
    stages::validate_lead_source(&mut conn, Some(&body.source))
        .await
        .map_err(ApiError::from)?;
    let contact_id = match body.contact_id.as_deref() {
        Some(t) => Some(pg_uuid(t)?),
        None => None,
    };
    let org_id = match body.organization_id.as_deref() {
        Some(t) => Some(pg_uuid(t)?),
        None => None,
    };
    let campaign_id = match body.source_campaign_id.as_deref() {
        Some(t) => Some(pg_uuid(t)?),
        None => None,
    };
    // exact from the request (13 Nov 2026: was f64, which also let NaN through)
    let estimated = body.estimated_value;
    let mut custom = json!({});
    let submitted = body.custom.clone().map(Value::Object);
    set_custom(&mut conn, "lead", &mut custom, submitted.as_ref()).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO leads (contact_id, organization_id, source, source_detail, source_campaign_id, title, \
         description, estimated_value, currency, interested_products, status, priority, assigned_to_id, \
         created_by_id, raw_message, attribution, custom, id, is_deleted, created_at, updated_at, segment) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, false, now(), now(), \
         $19)",
    )
    .bind(contact_id)
    .bind(org_id)
    .bind(&body.source)
    .bind(&body.source_detail)
    .bind(campaign_id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(estimated)
    .bind(&body.currency)
    .bind(&body.interested_products)
    .bind(&body.status)
    .bind(&body.priority)
    .bind(user.id)
    .bind(user.id)
    .bind(&body.raw_message)
    // JSONB None is the JSON null (none_as_null=False).
    .bind(
        body.attribution
            .clone()
            .map(Value::Object)
            .unwrap_or(Value::Null),
    )
    .bind(&custom)
    .bind(id)
    .bind(&segment)
    .execute(&mut *conn)
    .await?;
    granary_services::nary::events::emit(
        &mut conn,
        "lead.created",
        Some("lead"),
        Some(id),
        Some(json!({"source": body.source, "status": body.status, "origin": "rest"})),
        Some(user.id),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "title": body.title, "status": body.status, "segment": segment}),
    ))
}

/// `GET /leads/{lead_id}` (deleted leads included, as in Python).
pub async fn get_lead(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let id = pg_uuid(&lead_id)?;
    let lead: Option<Lead> =
        sqlx::query_as("SELECT * FROM leads WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(lead) = lead else {
        return Err(ApiError::not_found("Lead", Some(&lead_id)));
    };
    let creator_name: Option<String> = match lead.created_by_id {
        Some(c) => {
            sqlx::query_scalar("SELECT name FROM users WHERE id = $1")
                .bind(c)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    granary_services::crm::access::assert_can_view(&user, lead.assigned_to_id)?;
    let active = lead
        .status
        .as_deref()
        .is_some_and(|st| stages::ACTIVE_LEAD_STATUSES.contains(&st));
    let ids = if active { vec![lead.id] } else { vec![] };
    let needs_next = needs_next_action(&mut conn, "lead", &ids).await?;
    let lk = links(&mut conn, lead.contact_id, lead.organization_id).await?;
    Ok(resp::json(extend(
        lead_out(&lead, &needs_next),
        obj(vec![
            ("id", json!(lead.id.to_string())),
            ("title", json!(s(&lead.title))),
            ("status", json!(s(&lead.status))),
            ("priority", json!(s(&lead.priority))),
            ("source", json!(lead.source)),
            ("source_detail", json!(s(&lead.source_detail))),
            ("description", json!(s(&lead.description))),
            ("raw_message", json!(s(&lead.raw_message))),
            ("estimated_value", json!(float_if(&lead.estimated_value))),
            ("currency", json!(s(&lead.currency))),
            ("contact_id", json!(uid(lead.contact_id))),
            ("contact_name", json!(contact_name(&lk.contact))),
            (
                "contact_email",
                json!(contact_field(&lk.contact, |c| &c.email)),
            ),
            (
                "contact_phone",
                json!(contact_field(&lk.contact, |c| &c.phone)),
            ),
            (
                "contact_whatsapp",
                json!(contact_field(&lk.contact, |c| &c.whatsapp)),
            ),
            ("organization_name", json!(s(&lk.org_name))),
            ("organization_id", json!(uid(lead.organization_id))),
            ("assigned_to_id", json!(uid(lead.assigned_to_id))),
            ("created_by_id", json!(uid(lead.created_by_id))),
            ("created_by_name", json!(s(&creator_name))),
            ("created_at", json!(resp::iso(&lead.created_at))),
            (
                "ai_score",
                json!(lead.ai_score.clone().unwrap_or(Value::Null)),
            ),
            ("custom", json!(or_empty(&lead.custom))),
        ]),
    )))
}

/// `POST /leads/{lead_id}/score`: on-demand AI scoring of a lead.
pub async fn score_lead_endpoint(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
) -> ApiResult<Response> {
    use granary_services::ai::registry::{ai_ready, feature_enabled, get_ai_config};
    if !can_view_crm(&user) {
        return Err(ApiError::http(403, "Permission required: crm:view_own"));
    }
    let mut conn = db.conn().await?;
    let cfg = get_ai_config(&mut conn).await?;
    if !feature_enabled(&cfg, "scoring") {
        return Err(ApiError::http(
            400,
            "AI lead scoring is disabled in Settings \u{2192} Nary & AI \u{2192} AI features.",
        ));
    }
    if !ai_ready(&cfg, "scoring") {
        return Err(ApiError::http(
            400,
            "No API key for the AI provider lead scoring uses. Set it in Settings \u{2192} Nary & AI \u{2192} Providers & keys.",
        ));
    }
    let mut lead = load_lead(&mut conn, &lead_id).await?;
    granary_services::crm::access::assert_can_view(&user, lead.assigned_to_id)?;
    // Own-scope: users without view_all may only score their own leads.
    if !user.is_superadmin && !has(&user, "crm:view_all") && lead.assigned_to_id != Some(user.id) {
        return Err(ApiError::http(
            403,
            "You can only score leads assigned to you.",
        ));
    }
    let score = granary_services::ai::scoring::score_lead(&mut conn, &user, &mut lead).await?;
    if let Some(err) = score.get("error").filter(|e| py::truthy(e)) {
        let detail = match err {
            Value::String(t) => t.clone(),
            other => py::str_of(other),
        };
        return Err(ApiError::http(502, detail));
    }
    Ok(resp::json(score))
}

const LEAD_ALLOWED: [(&str, Kind); 16] = [
    ("title", Kind::Text),
    ("status", Kind::Text),
    ("priority", Kind::Text),
    ("source", Kind::Text),
    ("source_detail", Kind::Text),
    ("estimated_value", Kind::Num),
    ("currency", Kind::Text),
    ("description", Kind::Text),
    ("raw_message", Kind::Text),
    ("contact_id", Kind::Uuid),
    ("organization_id", Kind::Uuid),
    ("assigned_to_id", Kind::Uuid),
    ("disqualification_reason", Kind::Text),
    ("interested_products", Kind::TextArr),
    ("source_campaign_id", Kind::Uuid),
    ("attribution", Kind::Json),
];

/// `str(x or "")` of a body value against `str(row.x or "")`.
fn reassigned(body: &Map<String, Value>, current: Option<Uuid>) -> bool {
    match body.get("assigned_to_id") {
        None => false,
        Some(v) => {
            let new = if py::truthy(v) {
                py::str_of(v)
            } else {
                String::new()
            };
            new != current.map(|u| u.to_string()).unwrap_or_default()
        }
    }
}

fn str_or_repr(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => py::str_of(other),
    }
}

/// `PATCH /leads/{lead_id}`.
pub async fn update_lead(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
    PyJson(mut body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "crm:edit_lead")?;
    let mut conn = db.conn().await?;
    let lead = load_lead(&mut conn, &lead_id).await?;
    granary_services::crm::access::assert_can_view(&user, lead.assigned_to_id)?;
    let is_reassigned = reassigned(&body, lead.assigned_to_id);
    if is_reassigned && !user.is_superadmin && !has(&user, "crm:assign_leads") {
        return Err(ApiError::http(403, "Permission required: crm:assign_leads"));
    }
    let cur_status = lead.status.clone();
    let disqualifying = body.get("status") == Some(&json!("disqualified"))
        && cur_status.as_deref() != Some("disqualified");
    // Where a lead may move is services/crm/stages.py LEAD_TRANSITIONS; promotion is its own endpoint.
    if let Some(new) = body.get("status").cloned() {
        if new != cur_status.clone().map(Value::String).unwrap_or(Value::Null) {
            stages::validate_lead_transition(cur_status.as_deref(), &str_or_repr(&new))
                .map_err(ApiError::from)?;
            if new == json!("disqualified") {
                let reason = stages::validate_disqualify_reason(
                    &mut conn,
                    body.get("disqualification_reason"),
                )
                .await
                .map_err(ApiError::from)?;
                body.insert("disqualification_reason".into(), reason);
            }
        }
    }
    if let Some(src) = body.get("source").cloned() {
        if src != Value::String(lead.source.clone()) {
            let src_s = match &src {
                Value::Null => None,
                other => Some(str_or_repr(other)),
            };
            stages::validate_lead_source(&mut conn, src_s.as_deref())
                .await
                .map_err(|e: StageFail| ApiError::from(e))?;
        }
    }
    let row = row_map(&lead);
    let mut sets = collect_sets(&body, &LEAD_ALLOWED, &row)?;
    // the segment (services::segment); null or "" clears it
    if let Some(v) = body.get(granary_services::segment::FIELD) {
        let seg = segment_of(&mut conn, v).await?;
        if lead.segment != seg {
            sets.push((
                granary_services::segment::COLUMN.into(),
                seg.map(SqlVal::Text).unwrap_or(SqlVal::Null),
            ));
        }
    }
    let mut custom = lead.custom.clone();
    set_custom(&mut conn, "lead", &mut custom, body.get("custom")).await?;
    if custom != lead.custom {
        sets.push(("custom".into(), SqlVal::Json(custom)));
    }
    write_columns(&mut conn, "leads", lead.id, &sets).await?;
    // The values the in-memory row now holds.
    let now_val = |k: &str, cur: Value| body.get(k).cloned().unwrap_or(cur);
    let status = now_val("status", s(&lead.status));
    let title = now_val("title", s(&lead.title));
    if disqualifying {
        let reason = now_val("disqualification_reason", s(&lead.disqualification_reason));
        stages::recontact_reminder(
            &mut conn,
            "lead",
            lead.id,
            title.as_str(),
            "disqualified",
            &[reason],
            Some(user.id),
        )
        .await
        .map_err(ApiError::from)?;
    }
    let notified = if is_reassigned {
        let assignee = now_val("assigned_to_id", uid(lead.assigned_to_id));
        notify_assignment(&mut conn, &assignee, &user, "lead", lead.id, &title).await?
    } else {
        false
    };
    drop(conn);
    db.commit().await?;
    if notified {
        granary_services::approvals::notify::dispatch_outbox();
    }
    Ok(resp::json(
        json!({"id": lead.id.to_string(), "status": status}),
    ))
}

/// `POST /leads/{lead_id}/promote`: lead -> opportunity, idempotent.
pub async fn promote_lead(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "crm:create_deal")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&lead_id)?;
    let out = granary_services::crm::promote::promote_lead(&mut conn, id, &user, "promote").await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

/// `DELETE /leads/{lead_id}` (204).
pub async fn delete_lead(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
) -> ApiResult<Response> {
    if !has(&user, "crm:delete_contact") && !has(&user, "crm:edit_lead") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: crm:edit_lead"));
    }
    let mut conn = db.conn().await?;
    let lead = load_lead(&mut conn, &lead_id).await?;
    granary_services::crm::access::assert_can_view(&user, lead.assigned_to_id)?;
    soft_delete(&mut conn, "leads", lead.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Deals ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class DealCreate(BaseModel)`.
    pub struct DealCreate {
        pub title: String,
        pub lead_id: Option<String> = None,
        pub contact_id: Option<String> = None,
        pub organization_id: Option<String> = None,
        pub stage: String = "new",
        pub loss_reasons: Option<Vec<String>> = None,
        pub loss_notes: Option<String> = None,
        pub value: Option<Decimal> = None,
        pub currency: String = "USD",
        /// None = the stage's default (crm.funnel.stage_probabilities)
        pub probability: Option<i64> = None,
        pub expected_close_date: Option<String> = None,
        pub notes: Option<String> = None,
        pub product_line_ids: Vec<String> = Vec::new(),
        pub custom: Option<Map<String, Value>> = None,
        /// omitted: the lead's segment (services::segment)
        pub segment: Option<Value> = None,
    }
}

granary_core::py_model! {
    pub struct ListDealsQ {
        pub stage: Option<String> = None,
        /// a segment, or "none" for the deals without one
        pub segment: Option<String> = None,
        pub contact_id: Option<String> = None,
        pub organization_id: Option<String> = None,
        /// 10 Nov 2026: text search (title, organization, contact); `search` is read too
        pub q: Option<String> = None,
        pub search: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /deals`.
pub async fn list_deals(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListDealsQ>,
) -> ApiResult<Response> {
    crm_view_required(&user)?;
    let mut conn = db.conn().await?;
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM deals WHERE is_deleted = false");
    if !(user.is_superadmin || has(&user, "crm:view_all")) {
        qb.push(" AND assigned_to_id = ").push_bind(user.id);
    }
    if let Some(st) = q.stage.as_deref().filter(|t| !t.is_empty()) {
        qb.push(" AND stage = ").push_bind(st.to_string());
    }
    push_segment_filter(&mut qb, q.segment.as_deref());
    push_text_filter(&mut qb, q.q.as_deref(), q.search.as_deref());
    let cid = q.contact_id.as_deref().filter(|t| !t.is_empty());
    let oid = q.organization_id.as_deref().filter(|t| !t.is_empty());
    match (cid, oid) {
        (Some(c), Some(o)) => {
            qb.push(" AND (contact_id = ")
                .push_bind(pg_uuid(c)?)
                .push(" OR organization_id = ")
                .push_bind(pg_uuid(o)?)
                .push(")");
        }
        (Some(c), None) => {
            qb.push(" AND contact_id = ").push_bind(pg_uuid(c)?);
        }
        (None, Some(o)) => {
            qb.push(" AND organization_id = ").push_bind(pg_uuid(o)?);
        }
        (None, None) => {}
    }
    qb.push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let deals: Vec<Deal> = qb.build_query_as().fetch_all(&mut *conn).await?;
    let active: Vec<Uuid> = deals
        .iter()
        .filter(|d| stages::ACTIVE_STAGES.contains(&d.stage.as_str()))
        .map(|d| d.id)
        .collect();
    let needs_next = needs_next_action(&mut conn, "deal", &active).await?;
    let mut out = Vec::new();
    for d in &deals {
        let lk = links(&mut conn, d.contact_id, d.organization_id).await?;
        out.push(obj(vec![
            ("needs_next_action", json!(needs_next.contains(&d.id))),
            ("loss_reasons", json!(loss_reasons(&d.loss_reasons))),
            ("lead_id", json!(uid(d.lead_id))),
            ("id", json!(d.id.to_string())),
            ("title", json!(d.title)),
            ("stage", json!(d.stage)),
            ("value", json!(float_if(&d.value))),
            ("currency", json!(s(&d.currency))),
            ("probability", json!(d.probability)),
            (
                "expected_close_date",
                json!(resp::date_iso_opt(d.expected_close_date.as_ref())),
            ),
            ("contact_id", json!(uid(d.contact_id))),
            ("organization_id", json!(uid(d.organization_id))),
            ("contact_name", json!(contact_name(&lk.contact))),
            ("organization_name", json!(s(&lk.org_name))),
            ("notes", json!(s(&d.notes))),
            ("assigned_to_id", json!(uid(d.assigned_to_id))),
            ("product_line_ids", json!(arr(&d.product_line_ids))),
            ("segment", json!(s(&d.segment))),
        ]));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `d.loss_reasons or []`.
fn loss_reasons(v: &Option<Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => json!([]),
    }
}

/// `POST /deals` (201).
pub async fn create_deal(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<DealCreate>,
) -> ApiResult<Response> {
    need(&user, "crm:create_deal")?;
    let expected = match &body.expected_close_date {
        Some(t) => parse_date_field(&Value::String(t.clone()), "expected_close_date")?,
        None => None,
    };
    let mut conn = db.conn().await?;
    let segment = match &body.segment {
        Some(v) => segment_of(&mut conn, v).await?,
        None => None,
    };
    let mut custom = json!({});
    let submitted = body.custom.clone().map(Value::Object);
    set_custom(&mut conn, "deal", &mut custom, submitted.as_ref()).await?;
    let opt_uuid = |v: &Option<String>| -> ApiResult<Option<Uuid>> {
        match v.as_deref() {
            Some(t) => Ok(Some(pg_uuid(t)?)),
            None => Ok(None),
        }
    };
    let value = body.value;
    let now = Utc::now();
    let mut deal = Deal {
        lead_id: opt_uuid(&body.lead_id)?,
        contact_id: opt_uuid(&body.contact_id)?,
        organization_id: opt_uuid(&body.organization_id)?,
        title: body.title.clone(),
        deal_number: None,
        stage: "new".into(),
        value,
        currency: Some(body.currency.clone()),
        probability: None,
        expected_close_date: expected,
        actual_close_date: None,
        product_interests: None,
        product_line_ids: Some(body.product_line_ids.clone()),
        notes: body.notes.clone(),
        loss_reason: None,
        loss_reasons: None,
        loss_notes: body.loss_notes.clone(),
        assigned_to_id: Some(user.id),
        created_by_id: Some(user.id),
        deal_registration_id: None,
        custom,
        id: Uuid::new_v4(),
        created_at: now,
        updated_at: now,
        is_deleted: false,
        deleted_at: None,
        // omitted: insert_deal takes the lead's segment
        segment,
    };
    let reasons = body
        .loss_reasons
        .clone()
        .map(|r| Value::Array(r.into_iter().map(Value::String).collect()));
    stages::change_deal_stage(
        &mut conn,
        &mut deal,
        &body.stage,
        Some(user.id),
        "rest",
        reasons.as_ref(),
        body.loss_notes.as_deref(),
        None,
    )
    .await
    .map_err(ApiError::from)?;
    if let Some(p) = body.probability {
        let p32 = i32::try_from(p).map_err(|_| refused(&json!(p), "int"))?;
        if deal.probability != Some(p32) {
            sqlx::query("UPDATE deals SET probability = $2, updated_at = now() WHERE id = $1")
                .bind(deal.id)
                .bind(p32)
                .execute(&mut *conn)
                .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": deal.id.to_string(), "title": deal.title, "stage": deal.stage}),
    ))
}

/// `GET /deals/{deal_id}`.
pub async fn get_deal(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(deal_id): Path<String>,
) -> ApiResult<Response> {
    crm_view_required(&user)?;
    let mut conn = db.conn().await?;
    let deal = load_deal(&mut conn, &deal_id).await?;
    let lk = links(&mut conn, deal.contact_id, deal.organization_id).await?;
    if !(user.is_superadmin || has(&user, "crm:view_all")) && deal.assigned_to_id != Some(user.id) {
        return Err(ApiError::http(403, "This deal is assigned to someone else"));
    }
    let entered: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT max(at) FROM deal_stage_events WHERE deal_id = $1 AND to_stage = $2",
    )
    .bind(deal.id)
    .bind(&deal.stage)
    .fetch_one(&mut *conn)
    .await?;
    let quotes = sqlx::query(
        "SELECT id, full_number, number, status, total, currency, issue_date FROM quotations \
         WHERE deal_id = $1 AND is_deleted = false ORDER BY created_at DESC",
    )
    .bind(deal.id)
    .fetch_all(&mut *conn)
    .await?;
    let ids = if stages::ACTIVE_STAGES.contains(&deal.stage.as_str()) {
        vec![deal.id]
    } else {
        vec![]
    };
    let needs_next = needs_next_action(&mut conn, "deal", &ids).await?;
    let quotations: Vec<Value> = quotes
        .iter()
        .map(|q| {
            let full: Option<String> = q.get("full_number");
            let number: String = q.get("number");
            let total: Option<Decimal> = q.get("total");
            let issue: Option<NaiveDate> = q.get("issue_date");
            json!({
                "id": q.get::<Uuid, _>("id").to_string(),
                "number": match full { Some(f) if !f.is_empty() => f, _ => number },
                "status": q.get::<Option<String>, _>("status"),
                "total": resp::dec_float_opt(total.as_ref()),
                "currency": q.get::<Option<String>, _>("currency"),
                "issue_date": resp::date_iso_opt(issue.as_ref()),
            })
        })
        .collect();
    // R&D 26 Oct: the CRM inbox its new emails go from (null: the default)
    let mailbox_id: Option<Uuid> = sqlx::query_scalar("SELECT mailbox_id FROM deals WHERE id = $1")
        .bind(deal.id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json(obj(vec![
        ("mailbox_id", json!(uid(mailbox_id))),
        ("loss_reasons", json!(loss_reasons(&deal.loss_reasons))),
        ("stage_entered_at", json!(resp::iso_opt(entered.as_ref()))),
        ("needs_next_action", json!(needs_next.contains(&deal.id))),
        (
            "deal_registration_id",
            json!(uid(deal.deal_registration_id)),
        ),
        ("quotations", json!(quotations)),
        ("id", json!(deal.id.to_string())),
        ("title", json!(deal.title)),
        ("deal_number", json!(s(&deal.deal_number))),
        ("stage", json!(deal.stage)),
        ("probability", json!(deal.probability)),
        ("value", json!(float_if(&deal.value))),
        ("currency", json!(s(&deal.currency))),
        (
            "expected_close_date",
            json!(resp::date_iso_opt(deal.expected_close_date.as_ref())),
        ),
        (
            "actual_close_date",
            json!(resp::date_iso_opt(deal.actual_close_date.as_ref())),
        ),
        ("notes", json!(s(&deal.notes))),
        ("loss_reason", json!(s(&deal.loss_reason))),
        ("loss_notes", json!(s(&deal.loss_notes))),
        ("contact_id", json!(uid(deal.contact_id))),
        ("contact_name", json!(contact_name(&lk.contact))),
        ("organization_id", json!(uid(deal.organization_id))),
        ("organization_name", json!(s(&lk.org_name))),
        ("lead_id", json!(uid(deal.lead_id))),
        ("assigned_to_id", json!(uid(deal.assigned_to_id))),
        ("product_line_ids", json!(arr(&deal.product_line_ids))),
        ("created_at", json!(resp::iso(&deal.created_at))),
        ("custom", json!(or_empty(&deal.custom))),
        ("segment", json!(s(&deal.segment))),
    ])))
}

fn deal_kind(field: &str) -> Kind {
    match field {
        "probability" => Kind::Int,
        "value" => Kind::Num,
        "expected_close_date" | "actual_close_date" => Kind::Date,
        "contact_id" | "organization_id" | "assigned_to_id" => Kind::Uuid,
        "product_line_ids" => Kind::TextArr,
        _ => Kind::Text,
    }
}

/// `_apply_deal_updates(db, deal, body, allowed, user)`: the stage through stages.change_deal_stage, then
/// every other allowed key (an explicit probability or close date wins over what the stage set). Returns
/// the (column, value) pairs still to write.
async fn apply_deal_updates(
    conn: &mut PgConnection,
    deal: &mut Deal,
    body: &Map<String, Value>,
    allowed: &[&str],
    user: &AuthUser,
) -> ApiResult<Vec<(String, SqlVal)>> {
    let reasons = match body.get("loss_reasons") {
        Some(v) if py::truthy(v) => Some(v.clone()),
        _ => body.get("loss_reason").cloned(),
    };
    let stage_changes = match body.get("stage") {
        Some(st) if allowed.contains(&"stage") => *st != Value::String(deal.stage.clone()),
        _ => false,
    };
    if stage_changes {
        let close_date = match body.get("actual_close_date") {
            Some(v) if allowed.contains(&"actual_close_date") && py::truthy(v) => {
                parse_date_field(v, "actual_close_date")?
            }
            _ => None,
        };
        let note: Option<String> = match body.get("loss_notes") {
            None | Some(Value::Null) => None,
            Some(v) => Some(str_or_repr(v)),
        };
        stages::change_deal_stage(
            conn,
            deal,
            &str_or_repr(&body["stage"]),
            Some(user.id),
            "rest",
            reasons.as_ref(),
            note.as_deref(),
            close_date,
        )
        .await
        .map_err(ApiError::from)?;
    } else if reasons.as_ref().is_some_and(py::truthy)
        && stages::REASON_STAGES
            .iter()
            .any(|(st, _)| *st == deal.stage)
    {
        // Correcting the reasons of a deal that is already lost / disqualified.
        let clean = stages::validate_reasons(conn, &deal.stage.clone(), reasons.as_ref())
            .await
            .map_err(ApiError::from)?;
        let first = clean.first().map(py::str_of);
        sqlx::query(
            "UPDATE deals SET loss_reasons = $2, loss_reason = $3, updated_at = now() WHERE id = $1",
        )
        .bind(deal.id)
        .bind(Value::Array(clean.clone()))
        .bind(&first)
        .execute(&mut *conn)
        .await?;
        deal.loss_reasons = Some(Value::Array(clean));
        deal.loss_reason = first;
    }
    let row = row_map(&*deal);
    let mut sets: Vec<(String, SqlVal)> = Vec::new();
    for (k, v) in body {
        if !allowed.contains(&k.as_str())
            || matches!(k.as_str(), "stage" | "loss_reason" | "loss_reasons")
        {
            continue;
        }
        let val = if DEAL_DATE_FIELDS.contains(&k.as_str()) {
            match parse_date_field(v, k)? {
                Some(d) => SqlVal::Date(d),
                None => SqlVal::Null,
            }
        } else {
            col_val(deal_kind(k), v)?
        };
        sets.retain(|(c, _)| c != k);
        if !same_as_current(&row, k, &val) {
            sets.push((k.clone(), val));
        }
    }
    Ok(sets)
}

fn edit_deal_required(user: &AuthUser) -> ApiResult<()> {
    if !(user.is_superadmin || has(user, "crm:edit_deal") || has(user, "crm:edit_lead")) {
        return Err(ApiError::http(403, "Permission required: crm:edit_deal"));
    }
    Ok(())
}

const DEAL_ALLOWED: [&str; 13] = [
    "title",
    "stage",
    "probability",
    "value",
    "currency",
    "expected_close_date",
    "notes",
    "loss_notes",
    "actual_close_date",
    "contact_id",
    "organization_id",
    "product_line_ids",
    "assigned_to_id",
];

/// `PATCH /deals/{deal_id}`.
pub async fn update_deal(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(deal_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    edit_deal_required(&user)?;
    let mut conn = db.conn().await?;
    let mut deal = load_deal(&mut conn, &deal_id).await?;
    granary_services::crm::access::assert_can_view(&user, deal.assigned_to_id)?;
    let is_reassigned = reassigned(&body, deal.assigned_to_id);
    if is_reassigned && !user.is_superadmin && !has(&user, "crm:assign_leads") {
        return Err(ApiError::http(403, "Permission required: crm:assign_leads"));
    }
    let original_assignee = deal.assigned_to_id;
    let mut sets = apply_deal_updates(&mut conn, &mut deal, &body, &DEAL_ALLOWED, &user).await?;
    let mut custom = deal.custom.clone();
    set_custom(&mut conn, "deal", &mut custom, body.get("custom")).await?;
    if custom != deal.custom {
        sets.push(("custom".into(), SqlVal::Json(custom)));
    }
    // the segment (services::segment); null or "" clears it
    if let Some(v) = body.get(granary_services::segment::FIELD) {
        let seg = segment_of(&mut conn, v).await?;
        sets.push((
            granary_services::segment::COLUMN.into(),
            seg.map(SqlVal::Text).unwrap_or(SqlVal::Null),
        ));
    }
    write_columns(&mut conn, "deals", deal.id, &sets).await?;
    // R&D 26 Oct: the CRM inbox this deal's new emails go from (null: the default inbox)
    if let Some(v) = body.get("mailbox_id") {
        let mailbox = match v {
            Value::Null => None,
            Value::String(s) if s.trim().is_empty() => None,
            Value::String(s) => {
                let id = Uuid::parse_str(s.trim())
                    .map_err(|_| ApiError::http(422, "mailbox_id is not a CRM inbox"))?;
                match granary_services::crm_mailboxes::get(&mut conn, id).await? {
                    Some(m) if m.active => Some(m.id),
                    _ => return Err(ApiError::http(422, "mailbox_id is not an active CRM inbox")),
                }
            }
            _ => return Err(ApiError::http(422, "mailbox_id is not a CRM inbox")),
        };
        sqlx::query("UPDATE deals SET mailbox_id = $2, updated_at = now() WHERE id = $1")
            .bind(deal.id)
            .bind(mailbox)
            .execute(&mut *conn)
            .await?;
    }
    let title = body
        .get("title")
        .cloned()
        .unwrap_or_else(|| Value::String(deal.title.clone()));
    let notified = if is_reassigned {
        let assignee = body
            .get("assigned_to_id")
            .cloned()
            .unwrap_or_else(|| uid(original_assignee));
        notify_assignment(&mut conn, &assignee, &user, "deal", deal.id, &title).await?
    } else {
        false
    };
    drop(conn);
    db.commit().await?;
    if notified {
        granary_services::approvals::notify::dispatch_outbox();
    }
    Ok(resp::json(
        json!({"id": deal.id.to_string(), "title": title, "stage": deal.stage}),
    ))
}

const DEAL_STAGE_ALLOWED: [&str; 6] = [
    "stage",
    "probability",
    "value",
    "expected_close_date",
    "loss_notes",
    "actual_close_date",
];

/// `PATCH /deals/{deal_id}/stage`.
pub async fn update_deal_stage(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(deal_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    edit_deal_required(&user)?;
    let mut conn = db.conn().await?;
    let mut deal = load_deal(&mut conn, &deal_id).await?;
    granary_services::crm::access::assert_can_view(&user, deal.assigned_to_id)?;
    let sets = apply_deal_updates(&mut conn, &mut deal, &body, &DEAL_STAGE_ALLOWED, &user).await?;
    write_columns(&mut conn, "deals", deal.id, &sets).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": deal.id.to_string(), "stage": deal.stage}),
    ))
}

/// `DELETE /deals/{deal_id}` (204).
pub async fn delete_deal(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(deal_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "crm:delete_deal")?;
    let mut conn = db.conn().await?;
    let deal = load_deal(&mut conn, &deal_id).await?;
    granary_services::crm::access::assert_can_view(&user, deal.assigned_to_id)?;
    soft_delete(&mut conn, "deals", deal.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Activities ──────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class ActivityCreate(BaseModel)`.
    pub struct ActivityCreate {
        /// lead | deal | contact | organization
        pub entity_type: String,
        pub entity_id: String,
        pub contact_id: Option<String> = None,
        pub lead_id: Option<String> = None,
        pub deal_id: Option<String> = None,
        pub activity_type: String,
        pub direction: String = "outbound",
        pub subject: Option<String> = None,
        pub content: Option<String> = None,
        pub outcome: Option<String> = None,
        pub duration_minutes: Option<i64> = None,
        pub scheduled_at: Option<String> = None,
        pub channel_message_id: Option<String> = None,
    }
}

// 23 Oct: ACTIVITY_PARENTS and the visibility check live in granary_services::crm::access (shared with Nary).
use granary_services::crm::access::{assert_activity_visible, crm_sees_all, ACTIVITY_PARENTS};

/// `str.capitalize()` of one of the four known (ASCII) entity types.
fn capitalize(t: &str) -> String {
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// `POST /activities` (201).
pub async fn create_activity(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ActivityCreate>,
) -> ApiResult<Response> {
    need(&user, "crm:log_activity")?;
    let Some((_, table)) = ACTIVITY_PARENTS
        .iter()
        .find(|(k, _)| *k == body.entity_type)
    else {
        return Err(ApiError::http(
            400,
            "entity_type must be one of: contact, organization, lead, deal",
        ));
    };
    let entity_id = crate::extract::parse_uuid(&body.entity_id, "entity_id")?;
    let mut conn = db.conn().await?;
    let parent: Option<(Option<Uuid>,)> = sqlx::query_as(&format!(
        "SELECT assigned_to_id FROM {table} WHERE id = $1 AND is_deleted = false"
    ))
    .bind(entity_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((owner,)) = parent else {
        return Err(ApiError::not_found(
            &capitalize(&body.entity_type),
            Some(&body.entity_id),
        ));
    };
    granary_services::crm::access::assert_can_view(&user, owner)?;
    // 23 Oct: a note over crm.activity.max_chars is refused (422), never cut
    granary_services::crm::access::check_activity_length(&mut conn, body.content.as_deref())
        .await?;
    // Keep the typed FK in sync with the generic entity reference.
    let mut contact_id = body.contact_id.clone();
    let mut lead_id = body.lead_id.clone();
    let mut deal_id = body.deal_id.clone();
    let slot = match body.entity_type.as_str() {
        "contact" => Some(&mut contact_id),
        "lead" => Some(&mut lead_id),
        "deal" => Some(&mut deal_id),
        _ => None,
    };
    if let Some(slot) = slot {
        if slot.as_deref().is_none_or(str::is_empty) {
            *slot = Some(body.entity_id.clone());
        }
    }
    let scheduled: DateTime<Utc> = match body.scheduled_at.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => {
            let parsed =
                granary_services::crm::writes::py_datetime_fromisoformat(&t.replace('Z', "+00:00"))
                    .ok_or_else(|| {
                        ApiError::http(422, "scheduled_at must be a date and time (ISO 8601)")
                    })?;
            let (naive, off) = parsed;
            let off_secs = off.map(|(sec, _)| sec).unwrap_or(0);
            let off_micros = off.map(|(_, us)| us).unwrap_or(0);
            let utc = naive
                - chrono::Duration::seconds(off_secs)
                - chrono::Duration::microseconds(off_micros);
            DateTime::from_naive_utc_and_offset(utc, Utc)
        }
        None => Utc::now(),
    };
    let opt_uuid = |v: &Option<String>| -> ApiResult<Option<Uuid>> {
        match v.as_deref() {
            Some(t) => Ok(Some(pg_uuid(t)?)),
            None => Ok(None),
        }
    };
    let duration = match body.duration_minutes {
        Some(d) => Some(i32::try_from(d).map_err(|_| refused(&json!(d), "int"))?),
        None => None,
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO activities (entity_type, entity_id, contact_id, lead_id, deal_id, activity_type, direction, \
         subject, content, outcome, duration_minutes, scheduled_at, is_completed, channel_message_id, \
         created_by_id, id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, false, $13, $14, $15, now(), now())",
    )
    .bind(&body.entity_type)
    .bind(entity_id)
    .bind(opt_uuid(&contact_id)?)
    .bind(opt_uuid(&lead_id)?)
    .bind(opt_uuid(&deal_id)?)
    .bind(&body.activity_type)
    .bind(&body.direction)
    .bind(&body.subject)
    .bind(&body.content)
    .bind(&body.outcome)
    .bind(duration)
    .bind(scheduled)
    .bind(&body.channel_message_id)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"id": id.to_string()})))
}

granary_core::py_model! {
    pub struct ListActivitiesQ {
        pub entity_type: Option<String> = None,
        pub entity_id: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /activities`.
pub async fn list_activities(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListActivitiesQ>,
) -> ApiResult<Response> {
    crm_view_required(&user)?;
    let mut conn = db.conn().await?;
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT activities.*, users_1.name AS creator_name FROM activities \
         LEFT OUTER JOIN users AS users_1 ON activities.created_by_id = users_1.id WHERE true",
    );
    if let Some(et) = q.entity_type.as_deref().filter(|t| !t.is_empty()) {
        qb.push(" AND activities.entity_type = ")
            .push_bind(et.to_string());
    }
    if let Some(eid) = q.entity_id.as_deref().filter(|t| !t.is_empty()) {
        qb.push(" AND activities.entity_id = ")
            .push_bind(pg_uuid(eid)?);
    }
    if !crm_sees_all(&user) {
        // Own scope: activities of records that are unassigned or assigned to this user.
        qb.push(" AND (");
        for (i, (kind, table)) in ACTIVITY_PARENTS.iter().enumerate() {
            if i > 0 {
                qb.push(" OR ");
            }
            qb.push("(activities.entity_type = ")
                .push_bind(kind.to_string())
                .push(format!(
                    " AND activities.entity_id IN (SELECT {table}.id FROM {table} WHERE {table}.assigned_to_id IS NULL OR {table}.assigned_to_id = "
                ))
                .push_bind(user.id)
                .push("))");
        }
        qb.push(" OR (activities.entity_type NOT IN ('contact', 'organization', 'lead', 'deal') AND activities.created_by_id = ")
            .push_bind(user.id)
            .push("))");
    }
    qb.push(" ORDER BY activities.scheduled_at DESC LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let rows = qb.build().fetch_all(&mut *conn).await?;
    let mut out = Vec::new();
    for r in &rows {
        use sqlx::FromRow;
        let a = Activity::from_row(r)?;
        let creator: Option<String> = r.get("creator_name");
        out.push(activity_json(&a, &creator));
    }
    Ok(resp::json(Value::Array(out)))
}

/// One activity as `GET /activities` lists it (with its full content).
fn activity_json(a: &Activity, creator: &Option<String>) -> Value {
    obj(vec![
        ("id", json!(a.id.to_string())),
        ("entity_type", json!(a.entity_type)),
        ("entity_id", json!(a.entity_id.to_string())),
        ("activity_type", json!(a.activity_type)),
        ("direction", json!(s(&a.direction))),
        ("subject", json!(s(&a.subject))),
        ("content", json!(s(&a.content))),
        ("outcome", json!(s(&a.outcome))),
        ("duration_minutes", json!(a.duration_minutes)),
        (
            "scheduled_at",
            json!(resp::iso_opt(a.scheduled_at.as_ref())),
        ),
        (
            "completed_at",
            json!(resp::iso_opt(a.completed_at.as_ref())),
        ),
        ("is_completed", json!(a.is_completed)),
        ("created_by_id", json!(uid(a.created_by_id))),
        ("created_by_name", json!(s(creator))),
        ("created_at", json!(resp::iso(&a.created_at))),
    ])
}

/// 23 Oct: `GET /activities/{activity_id}`: one activity with its full content (the timeline shows only a
/// preview), under the same visibility rule as the list.
pub async fn get_activity(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(activity_id): Path<String>,
) -> ApiResult<Response> {
    crm_view_required(&user)?;
    let mut conn = db.conn().await?;
    let a = load_activity(&mut conn, &activity_id).await?;
    assert_activity_visible(&mut conn, &user, &a).await?;
    let creator: Option<String> = match a.created_by_id {
        Some(id) => sqlx::query_scalar::<_, Option<String>>("SELECT name FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .flatten(),
        None => None,
    };
    Ok(resp::json(activity_json(&a, &creator)))
}

granary_core::py_model! {
    /// `class ActivityUpdate(BaseModel)`.
    pub struct ActivityUpdate {
        pub subject: Option<String> = None,
        pub content: Option<String> = None,
        pub outcome: Option<String> = None,
    }
}

/// A model plus the keys the request set (`model_dump(exclude_unset=True)`).
pub struct WithSet<T>(pub T, pub Vec<String>);

impl<T: Validate> Validate for WithSet<T> {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let t = T::validate(v, cx)?;
        let keys = match v {
            PyObj::Dict(d) => d.keys().cloned().collect(),
            _ => vec![],
        };
        Some(WithSet(t, keys))
    }
}

async fn load_activity(conn: &mut PgConnection, activity_id: &str) -> ApiResult<Activity> {
    let id = crate::extract::parse_uuid(activity_id, "activity_id")?;
    sqlx::query_as::<_, Activity>("SELECT * FROM activities WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("Activity", Some(activity_id)))
}

/// `PATCH /activities/{activity_id}`.
pub async fn update_activity(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(activity_id): Path<String>,
    PyJson(WithSet(body, set)): PyJson<WithSet<ActivityUpdate>>,
) -> ApiResult<Response> {
    need(&user, "crm:log_activity")?;
    let mut conn = db.conn().await?;
    let a = load_activity(&mut conn, &activity_id).await?;
    assert_activity_visible(&mut conn, &user, &a).await?;
    if set.iter().any(|x| x == "content") {
        // 23 Oct: the same limit as a new note
        granary_services::crm::access::check_activity_length(&mut conn, body.content.as_deref())
            .await?;
    }
    let row = row_map(&a);
    let mut sets = Vec::new();
    for (k, v) in [
        ("subject", &body.subject),
        ("content", &body.content),
        ("outcome", &body.outcome),
    ] {
        if set.iter().any(|x| x == k) {
            let val = match v {
                Some(t) => SqlVal::Text(t.clone()),
                None => SqlVal::Null,
            };
            if !same_as_current(&row, k, &val) {
                sets.push((k.to_string(), val));
            }
        }
    }
    write_columns(&mut conn, "activities", a.id, &sets).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": a.id.to_string()})))
}

/// `DELETE /activities/{activity_id}` (204).
pub async fn delete_activity(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(activity_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "crm:log_activity")?;
    let mut conn = db.conn().await?;
    let a = load_activity(&mut conn, &activity_id).await?;
    assert_activity_visible(&mut conn, &user, &a).await?;
    // R&D 26 Oct: deleting voids it (kept with who and when, restorable by an admin)
    granary_services::crm::activity_void::void(&mut conn, &user, a.id, "deleted in the ERP", true)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// R&D 26 Oct: `GET /activities-deleted`: the deleted (voided) activities, newest first (the Owner, admins).
pub async fn list_voided_activities(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(
            403,
            "Only the Owner or an admin can see deleted activities",
        ));
    }
    let mut conn = db.conn().await?;
    let rows = granary_services::crm::activity_void::list(&mut conn, 200).await?;
    Ok(resp::json(json!({ "items": rows })))
}

/// R&D 26 Oct: `POST /activities-deleted/{voided_id}/restore`.
pub async fn restore_voided_activity(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(voided_id): Path<String>,
) -> ApiResult<Response> {
    let id = Uuid::parse_str(voided_id.trim())
        .map_err(|_| ApiError::http(404, "Deleted activity not found"))?;
    let mut conn = db.conn().await?;
    let out = granary_services::crm::activity_void::restore(&mut conn, &user, id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}
