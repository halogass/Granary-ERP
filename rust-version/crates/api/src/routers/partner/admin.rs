//! partner.py, internal side: accounts, portal invites, deal registrations, resources, commissions, partner
//! leads and the MDF review. Every permission check is in the handler body (after body validation).

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_core::validation::types::PyDec;
use granary_core::validation::v;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::crm::Deal;
use granary_db::rows::partner::{
    MDFClaim, MDFRequest, PartnerAccount, PartnerCommission, PartnerDealRegistration, PartnerLead,
    PartnerResource,
};
use granary_services::later::crm_deal_hooks::{apply_partner_deal_won, CLOSABLE_DEAL_STATUSES};

use super::{
    col_int, col_str, col_str_array, col_uuid, fromisoformat_or_422, get_truthy, hashable,
    one_or_none, py_500, py_repr, py_str, require_option, require_perm,
};
use crate::resp::{self, date_iso_opt, dec_float, dec_float_opt, iso, iso_opt};

/// `MONEY_MAX = Decimal("9999999999999999")` (Numeric(18, 2)).
pub(crate) const MONEY_MAX: &str = "9999999999999999";

// ─── Models ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class PartnerCreate(BaseModel)`.
    pub struct PartnerCreate {
        pub company_name: String,
        pub contact_name: Option<String> = None,
        pub contact_email: Option<String> = None,
        pub contact_phone: Option<String> = None,
        pub country: Option<String> = None,
        pub partner_type: String = "reseller",
        pub tier: String = "silver",
        pub website: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class DealRegistrationCreate(BaseModel)`.
    pub struct DealRegistrationCreate {
        pub partner_id: String,
        pub title: String,
        pub customer_org_name: Option<String> = None,
        pub customer_contact_name: Option<String> = None,
        pub customer_contact_email: Option<String> = None,
        pub customer_country: Option<String> = None,
        pub estimated_value: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX))),
        pub currency: String = "USD",
        pub expected_close_date: Option<String> = None,
        pub scope: Option<String> = None,
        pub products_services: Vec<String> = Vec::new(),
    }
}

granary_core::py_model! {
    /// `class DealCloseBody(BaseModel)`.
    pub struct DealCloseBody {
        /// won | lost
        pub outcome: String,
        pub final_value: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX))),
        /// YYYY-MM-DD, defaults to today; also the FX date
        pub closed_on: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class PartnerResourceCreate(BaseModel)`.
    pub struct PartnerResourceCreate {
        pub title: String,
        pub description: Option<String> = None,
        pub resource_type: String = "datasheet",
        pub file_url: Option<String> = None,
        pub min_tier: String = "authorized",
        pub version: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class CommissionCreate(BaseModel)`.
    pub struct CommissionCreate {
        pub partner_id: String,
        pub deal_registration_id: Option<String> = None,
        pub invoice_id: Option<String> = None,
        pub commission_type: String = "project_onetime",
        pub base_amount: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX))),
        /// a FRACTION, as stored: 0.1 = 10%
        pub rate: Decimal => v::decimal_c(v::DecC::new().ge("0").le("1")),
        pub currency: Option<String> = None,
        pub period_month: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("1").le("12"))),
        pub period_year: Option<i64> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `list_partners` query parameters.
    pub struct ListPartnersQ {
        pub partner_type: Option<String> = None,
        pub tier: Option<String> = None,
        /// 10 Nov 2026: the leads page asked for `status=active` and nothing read it
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    /// `partner_id` / `status` filters.
    pub struct PartnerStatusQ {
        pub partner_id: Option<String> = None,
        pub status: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `partner_id` / `status` filters with paging.
    pub struct PartnerStatusPageQ {
        pub partner_id: Option<String> = None,
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    /// `list_resources` query parameters.
    pub struct TierQ {
        pub tier: Option<String> = None,
    }
}

type Body = Map<String, Value>;

/// `if x:` on an optional query string.
fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// `select(PartnerAccount).where(PartnerAccount.id == partner_id, PartnerAccount.is_deleted == False)`
/// + `scalar_one_or_none()` + `NotFoundError("PartnerAccount", partner_id)`.
async fn live_partner(conn: &mut PgConnection, partner_id: &str) -> ApiResult<PartnerAccount> {
    let id = pg_uuid(partner_id)?;
    sqlx::query_as::<_, PartnerAccount>(
        "SELECT * FROM partner_accounts WHERE partner_accounts.id = $1 AND partner_accounts.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("PartnerAccount", Some(partner_id)))
}

/// `PartnerDealRegistration` by id (optionally `FOR UPDATE`) or `NotFoundError("DealRegistration", id)`.
async fn registration(
    conn: &mut PgConnection,
    reg_id: &str,
    for_update: bool,
) -> ApiResult<PartnerDealRegistration> {
    let id = pg_uuid(reg_id)?;
    let sql = if for_update {
        "SELECT * FROM partner_deal_registrations WHERE partner_deal_registrations.id = $1 FOR UPDATE"
    } else {
        "SELECT * FROM partner_deal_registrations WHERE partner_deal_registrations.id = $1"
    };
    sqlx::query_as::<_, PartnerDealRegistration>(sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("DealRegistration", Some(reg_id)))
}

/// INSERT of a new `PartnerDealRegistration` (register_deal / portal_create_deal): status pending, 90 days
/// of protection from now (UTC).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_registration(
    conn: &mut PgConnection,
    partner_id: Uuid,
    title: &str,
    customer_org_name: &Option<String>,
    customer_contact_name: &Option<String>,
    customer_contact_email: &Option<String>,
    customer_country: &Option<String>,
    estimated_value: Option<Decimal>,
    currency: &str,
    expected_close_date: &Option<String>,
    scope: &Option<String>,
    products_services: &[String],
) -> ApiResult<Uuid> {
    let now = Utc::now();
    let close = match expected_close_date.as_deref() {
        Some(s) if !s.is_empty() => Some(fromisoformat_or_422(s)?),
        _ => None,
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_deal_registrations (id, partner_id, customer_org_name, customer_contact_name, \
           customer_contact_email, customer_country, title, scope, estimated_value, currency, expected_close_date, \
           products_services, status, registered_at, protection_days, expires_at, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'pending', $13, 90, $14, now(), now())",
    )
    .bind(id)
    .bind(partner_id)
    .bind(customer_org_name)
    .bind(customer_contact_name)
    .bind(customer_contact_email)
    .bind(customer_country)
    .bind(title)
    .bind(scope)
    .bind(estimated_value)
    .bind(currency)
    .bind(close)
    .bind(products_services)
    .bind(now)
    .bind((now + Duration::days(90)).date_naive())
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

// ─── Accounts ─────────────────────────────────────────────────────────────────

/// `GET /accounts`.
pub async fn list_partners(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListPartnersQ>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:view")?;
    let mut conn = db.conn().await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT * FROM partner_accounts WHERE partner_accounts.is_deleted = false",
    );
    if let Some(t) = nonempty(&q.partner_type) {
        qb.push(" AND partner_accounts.partner_type = ")
            .push_bind(t);
    }
    if let Some(t) = nonempty(&q.tier) {
        qb.push(" AND partner_accounts.tier = ").push_bind(t);
    }
    if let Some(t) = nonempty(&q.status) {
        qb.push(" AND partner_accounts.status = ").push_bind(t);
    }
    qb.push(" ORDER BY partner_accounts.company_name LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let rows = qb
        .build_query_as::<PartnerAccount>()
        .fetch_all(&mut *conn)
        .await?;
    // Which partners have an active portal login: the page offers Invite or Revoke from it.
    let ids: Vec<Uuid> = rows.iter().map(|p| p.id).collect();
    let with_access: std::collections::HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT users.partner_account_id FROM users \
         WHERE users.partner_account_id = ANY($1) AND users.is_active = true",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    let out: Vec<Value> = rows
        .iter()
        .map(|p| {
            json!({
                "id": p.id.to_string(), "company_name": p.company_name, "partner_type": p.partner_type,
                "tier": p.tier, "status": p.status, "country": p.country,
                "contact_name": p.contact_name, "contact_email": p.contact_email,
                "created_at": iso(&p.created_at),
                "portal_access": with_access.contains(&p.id),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /accounts` (201).
pub async fn create_partner(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PartnerCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    require_option(super::PARTNER_TIERS, "tier", &body.tier)?;
    require_option(super::PARTNER_TYPES, "partner_type", &body.partner_type)?;
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    // PartnerAccount(**body.model_dump(), status="active", created_by_id=...): every Python-side column
    // default fires (empty arrays, zero metrics, USD commitment).
    sqlx::query(
        "INSERT INTO partner_accounts (id, company_name, contact_name, contact_email, contact_phone, country, \
           website, notes, partner_type, tier, status, territory_countries, territory_verticals, \
           authorized_products, authorized_services, is_exclusive, commitment_currency, certified_staff_count, \
           certification_details, cumulative_units, cumulative_revenue, current_year_units, current_year_revenue, \
           created_by_id, is_deleted, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'active', '{}', '{}', '{}', '{}', false, 'USD', 0, \
           '[]'::jsonb, 0, 0, 0, 0, $11, false, now(), now())",
    )
    .bind(id)
    .bind(&body.company_name)
    .bind(&body.contact_name)
    .bind(&body.contact_email)
    .bind(&body.contact_phone)
    .bind(&body.country)
    .bind(&body.website)
    .bind(&body.notes)
    .bind(&body.partner_type)
    .bind(&body.tier)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "company_name": body.company_name}),
    ))
}

/// `GET /accounts/{partner_id}`.
pub async fn get_partner(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(partner_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:view")?;
    let mut conn = db.conn().await?;
    let p = live_partner(&mut conn, &partner_id).await?;
    Ok(resp::json(json!({
        "id": p.id.to_string(), "company_name": p.company_name, "contact_name": p.contact_name,
        "contact_email": p.contact_email, "contact_phone": p.contact_phone, "country": p.country,
        "partner_type": p.partner_type, "tier": p.tier, "status": p.status,
        "website": p.website, "notes": p.notes,
    })))
}

/// `PATCH /accounts/{partner_id}`: change a partner's tier, type or status (any of them). Suspending or
/// terminating a partner also ends its portal logins; making it active again does not bring them back (invite
/// again).
pub async fn update_partner(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(partner_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let field = |key: &str| -> ApiResult<Option<String>> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
            Some(_) => Err(ApiError::http(400, format!("{key} must be a string"))),
        }
    };
    let tier = field("tier")?;
    let partner_type = field("partner_type")?;
    let status = field("status")?;
    if let Some(t) = &tier {
        require_option(super::PARTNER_TIERS, "tier", t)?;
    }
    if let Some(t) = &partner_type {
        require_option(super::PARTNER_TYPES, "partner_type", t)?;
    }
    if let Some(s) = &status {
        require_option(super::PARTNER_STATUSES, "status", s)?;
    }
    if tier.is_none() && partner_type.is_none() && status.is_none() {
        return Err(ApiError::http(
            400,
            "Send at least one of tier, partner_type or status",
        ));
    }
    let mut conn = db.conn().await?;
    let p = live_partner(&mut conn, &partner_id).await?;
    sqlx::query(
        "UPDATE partner_accounts SET tier = coalesce($2, tier), partner_type = coalesce($3, partner_type), \
           status = coalesce($4, status), updated_at = now() WHERE id = $1",
    )
    .bind(p.id)
    .bind(&tier)
    .bind(&partner_type)
    .bind(&status)
    .execute(&mut *conn)
    .await?;
    if matches!(status.as_deref(), Some("suspended") | Some("terminated")) {
        deactivate_portal_users(&mut conn, p.id).await?;
    }
    let p = live_partner(&mut conn, &partner_id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": p.id.to_string(), "company_name": p.company_name, "partner_type": p.partner_type,
        "tier": p.tier, "status": p.status,
    })))
}

/// Deactivates every active portal user linked to the partner.
async fn deactivate_portal_users(conn: &mut PgConnection, partner_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE users SET is_active = false, updated_at = now() \
         WHERE users.partner_account_id = $1 AND users.is_active = true",
    )
    .bind(partner_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `DELETE /accounts/{partner_id}` (204): soft delete, and its portal users can no longer log in.
pub async fn delete_partner(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(partner_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let p = live_partner(&mut conn, &partner_id).await?;
    sqlx::query(
        "UPDATE partner_accounts SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(p.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    deactivate_portal_users(&mut conn, p.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `DELETE /accounts/{partner_id}/access` (204): revoke portal access, keep the account.
pub async fn revoke_partner_access(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(partner_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let p = live_partner(&mut conn, &partner_id).await?;
    deactivate_portal_users(&mut conn, p.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `POST /accounts/{partner_id}/invite` (201): a portal login for the partner's contact (or the revoked one
/// reactivated with a fresh temporary password), emailed through the system SMTP.
pub async fn invite_partner_user(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(partner_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let p = live_partner(&mut conn, &partner_id).await?;
    let contact_email = match p.contact_email.as_deref() {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => {
            return Err(ApiError::http(
                400,
                "No contact email on file for this partner",
            ))
        }
    };
    let existing: Vec<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, is_active FROM users WHERE users.partner_account_id = $1 LIMIT 2",
    )
    .bind(p.id)
    .fetch_all(&mut *conn)
    .await?;
    let existing = one_or_none(existing)?;
    let temp_password = granary_compat::crypto::tokens::token_urlsafe(
        &mut granary_compat::crypto::tokens::OsRandom,
        12,
    );
    let display_name = match p.contact_name.as_deref() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => p.company_name.clone().unwrap_or_default(),
    };
    let hash = || {
        granary_compat::crypto::password::hash_password(&temp_password)
            .map_err(|e| py_500(format!("{e:?}")))
    };
    match existing {
        Some((_, true)) => {
            return Err(ApiError::http(
                409,
                "This partner already has portal access",
            ))
        }
        Some((uid, false)) => {
            // Reactivate the revoked user with fresh credentials.
            sqlx::query(
                "UPDATE users SET is_active = true, must_change_password = true, password_hash = $2, \
                 updated_at = now() WHERE id = $1",
            )
            .bind(uid)
            .bind(hash()?)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            // User(...) + flush: the email is unique (IntegrityError 409 when another user has it).
            // `name=partner.contact_name or partner.company_name`: a partner without either is a NOT NULL
            // violation, also 409.
            let name: Option<String> = match p.contact_name.as_deref() {
                Some(n) if !n.is_empty() => Some(n.to_string()),
                _ => p.company_name.clone(),
            };
            sqlx::query(
                "INSERT INTO users (id, email, name, password_hash, email_signature_enabled, is_active, \
                   is_superadmin, is_system_admin, portal_type, partner_account_id, must_change_password, \
                   password_changed_at, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, true, true, false, false, 'partner', $5, true, now(), now(), now())",
            )
            .bind(Uuid::new_v4())
            .bind(&contact_email)
            .bind(name)
            .bind(hash()?)
            .bind(p.id)
            .execute(&mut *conn)
            .await?;
        }
    }
    let smtp = granary_services::email::load_system_smtp(&mut conn)
        .await
        .map_err(py_500)?;
    let co = granary_core::company::get_company_info(&mut conn).await?;
    let html = granary_services::email_templates::portal_invite_email_template(
        &display_name,
        &contact_email,
        &temp_password,
        "partner",
        &format!("{}/login", co.app_url),
        &co.company_name,
        // smtp.get("from_email", co["company_email"]): the key is always there.
        &smtp.from_email,
    );
    granary_services::email::send_email(
        std::slice::from_ref(&contact_email),
        &format!("Your Partner Portal Access - {}", co.company_name),
        &html,
        &mut conn,
        None,
        None,
    )
    .await
    .map_err(py_500)?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"email": contact_email, "message": "Invite sent successfully"}),
    ))
}

// ─── Deal registrations ───────────────────────────────────────────────────────

/// `POST /deal-registrations` (201).
pub async fn register_deal(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<DealRegistrationCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    // The close date is parsed when the object is built, the partner id bound at the flush.
    if let Some(s) = body
        .expected_close_date
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        fromisoformat_or_422(s)?;
    }
    let partner_id = pg_uuid(&body.partner_id)?;
    let id = insert_registration(
        &mut conn,
        partner_id,
        &body.title,
        &body.customer_org_name,
        &body.customer_contact_name,
        &body.customer_contact_email,
        &body.customer_country,
        body.estimated_value,
        &body.currency,
        &body.expected_close_date,
        &body.scope,
        &body.products_services,
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "pending"}),
    ))
}

/// `GET /deal-registrations`.
pub async fn list_deal_registrations(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<PartnerStatusQ>,
) -> ApiResult<Response> {
    if !(user.is_superadmin
        || user.perms.contains("partner_mgmt:view")
        || user.perms.contains("partner_mgmt:approve_deals"))
    {
        return Err(ApiError::http(403, "Insufficient permissions"));
    }
    let mut conn = db.conn().await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT partner_deal_registrations.*, partner_accounts.company_name AS partner_company_name \
         FROM partner_deal_registrations LEFT OUTER JOIN partner_accounts \
         ON partner_accounts.id = partner_deal_registrations.partner_id",
    );
    let mut sep = " WHERE ";
    if let Some(pid) = nonempty(&q.partner_id) {
        qb.push(sep)
            .push("partner_deal_registrations.partner_id = ")
            .push_bind(pg_uuid(pid)?);
        sep = " AND ";
    }
    if let Some(s) = nonempty(&q.status) {
        qb.push(sep)
            .push("partner_deal_registrations.status = ")
            .push_bind(s.to_string());
    }
    qb.push(" ORDER BY partner_deal_registrations.created_at DESC");
    let rows = qb.build().fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let r = <PartnerDealRegistration as sqlx::FromRow<_>>::from_row(row)?;
        let partner_name: Option<String> = row.try_get("partner_company_name")?;
        out.push(json!({
            "id": r.id.to_string(), "partner_id": r.partner_id.to_string(), "partner_name": partner_name,
            "title": r.title, "customer_org_name": r.customer_org_name,
            "estimated_value": dec_float_opt(r.estimated_value.as_ref()),
            "currency": r.currency,
            "expected_close_date": date_iso_opt(r.expected_close_date.as_ref()),
            "status": r.status, "rejection_reason": r.rejection_reason,
            "expires_at": date_iso_opt(r.expires_at.as_ref()),
            "extended_until": date_iso_opt(r.extended_until.as_ref()),
            "created_at": iso(&r.created_at),
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `PATCH /deal-registrations/{reg_id}/review`: approve (linking or creating the CRM deal) or reject.
pub async fn review_deal_registration(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(reg_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:approve_deals")?;
    let mut conn = db.conn().await?;
    let mut reg = registration(&mut conn, &reg_id, false).await?;
    if reg.status.as_deref() != Some("pending") {
        return Err(ApiError::http(
            400,
            format!(
                "Only pending registrations can be reviewed (current: {})",
                reg.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    // Accept {"decision": "approved"|"rejected"} or {"action": "approve"|"reject"}; never default.
    let raw = get_truthy(&body, "decision")
        .or_else(|| body.get("action"))
        .cloned()
        .unwrap_or(Value::Null);
    if !hashable(&raw) {
        return Err(py_500(format!("unhashable type: {}", py_repr(&raw))));
    }
    let decision = match raw.as_str() {
        Some("approved") | Some("approve") => "approved",
        Some("rejected") | Some("reject") => "rejected",
        _ => {
            return Err(ApiError::http(
                400,
                "decision must be 'approved' or 'rejected'",
            ))
        }
    };
    let approved = decision == "approved";
    let rejection_reason = if approved {
        None
    } else {
        col_str(get_truthy(&body, "reason").or_else(|| body.get("notes")))?
    };
    reg.status = Some(decision.to_string());
    let deal_id = if approved {
        Some(link_registration_to_deal(&mut conn, &mut reg, body.get("deal_id"), &user).await?)
    } else {
        None
    };
    sqlx::query(
        "UPDATE partner_deal_registrations SET status = $2, approved_by_id = $3, approved_at = $4, \
           rejection_reason = $5, deal_id = $6, updated_at = now() WHERE id = $1",
    )
    .bind(reg.id)
    .bind(&reg.status)
    .bind(if approved { Some(user.id) } else { None })
    .bind(if approved { Some(Utc::now()) } else { None })
    .bind(&rejection_reason)
    .bind(reg.deal_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": reg.id.to_string(), "status": reg.status,
        "deal_id": deal_id.map(|d: Uuid| d.to_string()),
    })))
}

/// `_link_registration_to_deal(db, reg, deal_id, user)`: an approved registration belongs to a deal: the
/// one the reviewer picked, or a new one at `new` built from the registration and owned by the approver.
/// Both sides are linked. Returns the deal id (sets `reg.deal_id`; the caller writes the registration).
async fn link_registration_to_deal(
    conn: &mut PgConnection,
    reg: &mut PartnerDealRegistration,
    deal_id: Option<&Value>,
    user: &AuthUser,
) -> ApiResult<Uuid> {
    let deal_id = deal_id.filter(|v| super::truthy(v));
    let deal_uuid = if let Some(raw) = deal_id {
        // parse_uuid(deal_id, "deal_id"): uuid.UUID(val) of a non-str raises AttributeError -> 400.
        let parsed = match raw {
            Value::String(s) => crate::extract::parse_uuid(s, "deal_id")?,
            _ => return Err(ApiError::http(400, "Invalid deal_id format")),
        };
        let found: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT id, deal_registration_id FROM deals WHERE deals.id = $1 AND deals.is_deleted = false",
        )
        .bind(parsed)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((id, linked)) = found else {
            return Err(ApiError::http(400, "deal_id: no such deal"));
        };
        if linked.is_some_and(|l| l != reg.id) {
            return Err(ApiError::http(
                400,
                "That deal is already linked to another partner registration",
            ));
        }
        if linked != Some(reg.id) {
            sqlx::query(
                "UPDATE deals SET deal_registration_id = $2, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(reg.id)
            .execute(&mut *conn)
            .await?;
        }
        id
    } else {
        let label = match reg.registration_number.as_deref() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => reg.id.to_string(),
        };
        let for_org = match reg.customer_org_name.as_deref() {
            Some(n) if !n.is_empty() => format!(" for {n}"),
            _ => String::new(),
        };
        let now = Utc::now();
        let mut deal = Deal {
            lead_id: None,
            contact_id: None,
            organization_id: reg.customer_org_id,
            title: reg.title.clone(),
            deal_number: None,
            stage: "new".into(),
            value: reg.estimated_value,
            currency: reg.currency.clone(),
            probability: None,
            expected_close_date: reg.expected_close_date,
            actual_close_date: None,
            product_interests: None,
            product_line_ids: None,
            notes: Some(format!("From partner deal registration {label}{for_org}.")),
            loss_reason: None,
            loss_reasons: None,
            loss_notes: None,
            assigned_to_id: Some(user.id),
            created_by_id: Some(user.id),
            // `deal.deal_registration_id = reg.id` right after the stage change (same transaction).
            deal_registration_id: Some(reg.id),
            custom: json!({}),
            id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            is_deleted: false,
            deleted_at: None,
            // 5 Nov 2026: core until its quotation says otherwise (insert_deal, automatic)
            segment: None,
        };
        granary_services::crm::stages::change_deal_stage(
            conn,
            &mut deal,
            "new",
            Some(user.id),
            "partner",
            None,
            None,
            None,
        )
        .await?;
        deal.id
    };
    reg.deal_id = Some(deal_uuid);
    Ok(deal_uuid)
}

/// `PATCH /deal-registrations/{reg_id}/close`: close an approved registration as won or lost. A win adds the
/// deal value to the partner's revenue (in its commitment currency).
pub async fn close_deal_registration(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(reg_id): Path<String>,
    PyJson(body): PyJson<DealCloseBody>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:approve_deals")?;
    if body.outcome != "won" && body.outcome != "lost" {
        return Err(ApiError::http(400, "outcome must be 'won' or 'lost'"));
    }
    let mut conn = db.conn().await?;
    let closed_on = match body.closed_on.as_deref() {
        Some(s) if !s.is_empty() => {
            match granary_services::crm::attributes::py::date_fromisoformat(s) {
                Some(d) => d,
                None => return Err(ApiError::http(422, "closed_on must be a date (YYYY-MM-DD)")),
            }
        }
        _ => granary_core::clock::today_local(&mut conn).await?,
    };
    let mut reg = registration(&mut conn, &reg_id, true).await?;
    let status = reg.status.clone();
    if !status
        .as_deref()
        .is_some_and(|s| CLOSABLE_DEAL_STATUSES.contains(&s))
    {
        return Err(ApiError::http(
            400,
            format!(
                "A {} registration can't be closed (allowed: {})",
                status.as_deref().unwrap_or("None"),
                CLOSABLE_DEAL_STATUSES.join(", ")
            ),
        ));
    }
    let mut added = None;
    if body.outcome == "won" {
        added = Some(
            apply_partner_deal_won(&mut conn, &mut reg, body.final_value, Some(closed_on)).await?,
        );
        if let Some(v) = body.final_value {
            reg.estimated_value = Some(v); // the registration now carries the closing value
        }
        reg.status = Some("closed_won".into());
    } else {
        reg.status = Some("closed_lost".into());
    }
    sqlx::query(
        "UPDATE partner_deal_registrations SET estimated_value = $2, status = $3, revenue_credited = $4, \
           revenue_credited_currency = $5, revenue_credited_year = $6, status_before_close = $7, \
           updated_at = now() WHERE id = $1",
    )
    .bind(reg.id)
    .bind(reg.estimated_value)
    .bind(&reg.status)
    .bind(reg.revenue_credited)
    .bind(&reg.revenue_credited_currency)
    .bind(reg.revenue_credited_year)
    .bind(&reg.status_before_close)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": reg.id.to_string(), "status": reg.status,
        "revenue_added": dec_float_opt(added.as_ref()),
    })))
}

/// `PATCH /deal-registrations/{reg_id}/protection`: `{action: "expire"}` ends an approved (or extended)
/// registration's protection now; `{action: "extend", until: "YYYY-MM-DD"}` extends it to a later date.
pub async fn update_registration_protection(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(reg_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:approve_deals")?;
    let action = body.get("action").and_then(Value::as_str).unwrap_or("");
    if action != "expire" && action != "extend" {
        return Err(ApiError::http(400, "action must be 'expire' or 'extend'"));
    }
    let mut conn = db.conn().await?;
    let reg = registration(&mut conn, &reg_id, true).await?;
    let current = reg.status.as_deref().unwrap_or("None");
    if current != "approved" && current != "extended" {
        return Err(ApiError::http(
            400,
            format!("Only approved or extended registrations can be expired or extended (current: {current})"),
        ));
    }
    let today = Utc::now().date_naive();
    if action == "expire" {
        sqlx::query(
            "UPDATE partner_deal_registrations SET status = 'expired', expires_at = least(coalesce(expires_at, $2), $2), \
               updated_at = now() WHERE id = $1",
        )
        .bind(reg.id)
        .bind(today)
        .execute(&mut *conn)
        .await?;
    } else {
        let until = match body.get("until").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => fromisoformat_or_422(s)?,
            _ => {
                return Err(ApiError::http(
                    400,
                    "until (YYYY-MM-DD) is required to extend",
                ))
            }
        };
        let ends = reg.extended_until.or(reg.expires_at).unwrap_or(today);
        if until <= ends.max(today) {
            return Err(ApiError::http(
                400,
                format!(
                    "until must be after the current end of protection ({})",
                    ends.max(today)
                ),
            ));
        }
        sqlx::query(
            "UPDATE partner_deal_registrations SET status = 'extended', extended_until = $2, updated_at = now() \
             WHERE id = $1",
        )
        .bind(reg.id)
        .bind(until)
        .execute(&mut *conn)
        .await?;
    }
    let reg = registration(&mut conn, &reg_id, false).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": reg.id.to_string(), "status": reg.status,
        "expires_at": date_iso_opt(reg.expires_at.as_ref()),
        "extended_until": date_iso_opt(reg.extended_until.as_ref()),
    })))
}

/// `DELETE /deal-registrations/{reg_id}` (204): pending registrations only.
pub async fn delete_deal_registration(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(reg_id): Path<String>,
) -> ApiResult<Response> {
    if !(user.is_superadmin
        || user.perms.contains("partner_mgmt:manage")
        || user.perms.contains("partner_mgmt:approve_deals"))
    {
        return Err(ApiError::http(403, "Insufficient permissions"));
    }
    let mut conn = db.conn().await?;
    let reg = registration(&mut conn, &reg_id, false).await?;
    if reg.status.as_deref() != Some("pending") {
        return Err(ApiError::http(
            400,
            "Only pending deal registrations can be deleted",
        ));
    }
    sqlx::query("DELETE FROM partner_deal_registrations WHERE partner_deal_registrations.id = $1")
        .bind(reg.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Resources ────────────────────────────────────────────────────────────────

/// `GET /resources`: active resources. Staff see them all; a partner portal account sees only those of
/// its own tier and lower tiers, for every partner type or its own type (security triage #20); other
/// portal accounts (vendor, customer) are refused.
pub async fn list_resources(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<TierQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let partner = if user.is_internal() {
        None
    } else if user.portal_type == "partner" {
        Some(super::portal::portal_partner(&mut conn, &user).await?)
    } else {
        return Err(ApiError::http(403, "Internal users only"));
    };
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT * FROM partner_resources WHERE partner_resources.is_active = true",
    );
    if let Some(t) = nonempty(&q.tier) {
        qb.push(" AND partner_resources.min_tier = ").push_bind(t);
    }
    qb.push(" ORDER BY partner_resources.resource_type, partner_resources.title");
    let rows = qb
        .build_query_as::<PartnerResource>()
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = rows
        .iter()
        .filter(|r| {
            partner
                .as_ref()
                .is_none_or(|p| super::portal::resource_visible_to(r, p))
        })
        .map(|r| {
            json!({
                "id": r.id.to_string(), "title": r.title, "description": r.description,
                "resource_type": r.resource_type, "min_tier": r.min_tier,
                "file_url": r.file_url, "version": r.version,
                "download_count": r.download_count, "is_active": r.is_active,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /resources` (201).
pub async fn create_resource(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PartnerResourceCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_resources (id, title, description, resource_type, file_url, version, min_tier, \
           partner_types, is_active, download_count, created_by_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, '{}', true, 0, $8, now(), now())",
    )
    .bind(id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(&body.resource_type)
    .bind(&body.file_url)
    .bind(&body.version)
    .bind(&body.min_tier)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "title": body.title}),
    ))
}

async fn resource_id(conn: &mut PgConnection, resource_id: &str) -> ApiResult<Uuid> {
    let id = pg_uuid(resource_id)?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM partner_resources WHERE partner_resources.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    found
        .map(|(id,)| id)
        .ok_or_else(|| ApiError::not_found("PartnerResource", Some(resource_id)))
}

/// `DELETE /resources/{resource_id}` (204).
pub async fn delete_resource(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rid): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = resource_id(&mut conn, &rid).await?;
    sqlx::query("DELETE FROM partner_resources WHERE partner_resources.id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// One column a `setattr` loop may write, with its SQL type.
#[derive(Clone, Copy)]
pub(crate) enum ColKind {
    Str,
    Bool,
    Int,
    StrArray,
}

/// `for k, v in body.items(): if k in allowed: setattr(obj, k, v)`, then the flush: one UPDATE of the
/// allowed keys present (asyncpg refuses a wrong type: 500; NULL in a NOT NULL column: 409).
pub(crate) async fn setattr_update(
    conn: &mut PgConnection,
    table: &str,
    id: Uuid,
    body: &Body,
    allowed: &[(&str, ColKind)],
) -> ApiResult<()> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(format!("UPDATE {table} SET "));
    let mut any = false;
    for (k, v) in body {
        let Some((col, kind)) = allowed.iter().find(|(c, _)| c == k) else {
            continue;
        };
        if any {
            qb.push(", ");
        }
        any = true;
        qb.push(format!("\"{col}\" = "));
        match kind {
            ColKind::Str => {
                qb.push_bind(col_str(Some(v))?);
            }
            ColKind::Bool => {
                qb.push_bind(super::col_bool(Some(v))?);
            }
            ColKind::Int => {
                qb.push_bind(col_int(Some(v))?);
            }
            ColKind::StrArray => {
                qb.push_bind(col_str_array(Some(v))?);
            }
        }
    }
    if !any {
        return Ok(());
    }
    qb.push(", updated_at = now() WHERE id = ").push_bind(id);
    qb.build().execute(&mut *conn).await?;
    Ok(())
}

/// `PATCH /resources/{resource_id}`.
pub async fn update_resource(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rid): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = resource_id(&mut conn, &rid).await?;
    setattr_update(
        &mut conn,
        "partner_resources",
        id,
        &body,
        &[
            ("title", ColKind::Str),
            ("description", ColKind::Str),
            ("resource_type", ColKind::Str),
            ("file_url", ColKind::Str),
            ("min_tier", ColKind::Str),
            ("version", ColKind::Str),
            ("is_active", ColKind::Bool),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": id.to_string()})))
}

// ─── Commissions ──────────────────────────────────────────────────────────────

/// `GET /commissions`.
pub async fn list_commissions(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<PartnerStatusQ>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:view")?;
    let mut conn = db.conn().await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT partner_commissions.*, partner_accounts.company_name AS partner_company_name \
         FROM partner_commissions LEFT OUTER JOIN partner_accounts \
         ON partner_accounts.id = partner_commissions.partner_id",
    );
    let mut sep = " WHERE ";
    if let Some(pid) = nonempty(&q.partner_id) {
        qb.push(sep)
            .push("partner_commissions.partner_id = ")
            .push_bind(pg_uuid(pid)?);
        sep = " AND ";
    }
    if let Some(s) = nonempty(&q.status) {
        qb.push(sep)
            .push("partner_commissions.status = ")
            .push_bind(s.to_string());
    }
    qb.push(" ORDER BY partner_commissions.created_at DESC");
    let rows = qb.build().fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let c = <PartnerCommission as sqlx::FromRow<_>>::from_row(row)?;
        let partner_name: Option<String> = row.try_get("partner_company_name")?;
        out.push(json!({
            "id": c.id.to_string(), "partner_id": c.partner_id.to_string(), "partner_name": partner_name,
            "deal_registration_id": c.deal_registration_id.map(|d| d.to_string()),
            "commission_type": c.commission_type,
            "base_amount": dec_float(&c.base_amount), "rate": dec_float(&c.rate), "amount": dec_float(&c.amount),
            "currency": c.currency, "status": c.status,
            "paid_at": iso_opt(c.paid_at.as_ref()),
            "created_at": iso(&c.created_at),
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `_require_any(current_user, *codes)`: superadmin, or any of the codes; 403 names the first.
pub(crate) fn require_any(user: &AuthUser, codes: &[&str]) -> ApiResult<()> {
    if user.is_superadmin || codes.iter().any(|c| user.perms.contains(*c)) {
        Ok(())
    } else {
        Err(ApiError::http(
            403,
            format!("Permission required: {}", codes[0]),
        ))
    }
}

/// `POST /commissions` (201): amount = base_amount x rate, computed here.
pub async fn create_commission(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CommissionCreate>,
) -> ApiResult<Response> {
    require_any(&user, &["partner_mgmt:manage_commissions"])?;
    let mut conn = db.conn().await?;
    let partner = live_partner(&mut conn, &body.partner_id).await?;
    let mut base = body.base_amount;
    let mut currency = body.currency.clone();
    if let Some(drid) = body
        .deal_registration_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let id = pg_uuid(drid)?;
        let reg = sqlx::query_as::<_, PartnerDealRegistration>(
            "SELECT * FROM partner_deal_registrations WHERE partner_deal_registrations.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        let reg = match reg {
            Some(r) if r.partner_id == partner.id => r,
            _ => {
                return Err(ApiError::http(
                    400,
                    "That deal registration is not this partner's",
                ))
            }
        };
        if base.is_none() {
            base = reg.estimated_value;
        }
        match currency.as_deref() {
            None | Some("") => currency = reg.currency.clone(),
            Some(c) => {
                if let Some(rc) = reg.currency.as_deref().filter(|s| !s.is_empty()) {
                    if c.to_uppercase() != rc.to_uppercase() && body.base_amount.is_none() {
                        return Err(ApiError::http(
                            400,
                            format!("The deal is in {rc}; give base_amount in {c}"),
                        ));
                    }
                }
            }
        }
    }
    let Some(base) = base else {
        return Err(ApiError::http(400, "base_amount is required"));
    };
    let currency = match currency.as_deref() {
        Some(c) if !c.is_empty() => c.to_uppercase(),
        _ => "USD".to_string(),
    };
    let places = currency_places(&mut conn, Some(&currency)).await?;
    let base = quantize_money(base, places);
    let amount = quantize_money(base * D(body.rate), places);
    // deal_registration_id / invoice_id are bound as given (an empty string is a DataError at the flush).
    let deal_registration_id = match body.deal_registration_id.as_deref() {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let invoice_id = match body.invoice_id.as_deref() {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_commissions (id, partner_id, deal_registration_id, invoice_id, commission_type, \
           base_amount, rate, amount, currency, period_month, period_year, status, notes, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'pending', $12, now(), now())",
    )
    .bind(id)
    .bind(partner.id)
    .bind(deal_registration_id)
    .bind(invoice_id)
    .bind(&body.commission_type)
    .bind(base)
    .bind(body.rate)
    .bind(amount)
    .bind(&currency)
    .bind(body.period_month)
    .bind(body.period_year)
    .bind(&body.notes)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "pending", "amount": dec_float(&amount), "currency": currency}),
    ))
}

/// PartnerCommission.status values; paid and cancelled are final.
const COMMISSION_STATUSES: &[&str] = &["pending", "approved", "paid", "cancelled"];

fn commission_transitions(from: Option<&str>) -> &'static [&'static str] {
    match from {
        Some("pending") => &["approved", "cancelled"],
        Some("approved") => &["paid", "cancelled"],
        _ => &[],
    }
}

/// `PATCH /commissions/{commission_id}/status`: approve, cancel or mark paid.
pub async fn update_commission_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(commission_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    let new_status = body.get("status").cloned().unwrap_or(Value::Null);
    let new = new_status
        .as_str()
        .filter(|s| COMMISSION_STATUSES.contains(s));
    let Some(new) = new else {
        return Err(ApiError::http(
            400,
            format!("status must be one of: {}", COMMISSION_STATUSES.join(", ")),
        ));
    };
    // Approving and paying are separate rights.
    match new {
        "approved" | "cancelled" => require_any(&user, &["partner_mgmt:approve_commission"])?,
        "paid" => require_any(&user, &["partner_mgmt:pay_commission"])?,
        _ => {}
    }
    let mut conn = db.conn().await?;
    let id = pg_uuid(&commission_id)?;
    // Locked, and the old status read after the lock.
    let comm = sqlx::query_as::<_, PartnerCommission>(
        "SELECT * FROM partner_commissions WHERE partner_commissions.id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("PartnerCommission", Some(&commission_id)))?;
    let old = comm.status.as_deref();
    let allowed = commission_transitions(old);
    if !allowed.contains(&new) {
        let mut detail = format!(
            "Cannot move a {} commission to {}",
            old.unwrap_or("None"),
            py_repr(&new_status)
        );
        if !allowed.is_empty() {
            detail.push_str(&format!(" (allowed: {})", allowed.join(", ")));
        }
        return Err(ApiError::http(400, detail));
    }
    match new {
        "approved" => {
            sqlx::query(
                "UPDATE partner_commissions SET status = $2, approved_by_id = $3, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(new)
            .bind(user.id)
            .execute(&mut *conn)
            .await?;
        }
        "paid" => {
            let reference = col_str(body.get("payment_reference"))?;
            sqlx::query(
                "UPDATE partner_commissions SET status = $2, paid_at = $3, payment_reference = $4, \
                 updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(new)
            .bind(Utc::now())
            .bind(reference)
            .execute(&mut *conn)
            .await?;
        }
        _ => {
            sqlx::query(
                "UPDATE partner_commissions SET status = $2, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(new)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": id.to_string(), "status": new})))
}

// ─── Partner leads (company admin) ────────────────────────────────────────────

/// `_lead_dict(l)`.
pub(crate) fn lead_dict(l: &PartnerLead) -> Value {
    json!({
        "id": l.id.to_string(),
        "partner_id": l.partner_id.to_string(),
        "customer_name": l.customer_name,
        "customer_email": l.customer_email,
        "customer_company": l.customer_company,
        "customer_country": l.customer_country,
        "product_interest": l.product_interest.clone().unwrap_or_default(),
        "notes": l.notes,
        "status": l.status,
        "assigned_at": iso_opt(l.assigned_at.as_ref()),
        "expires_at": iso_opt(l.expires_at.as_ref()),
        "created_at": iso(&l.created_at),
    })
}

/// `body[key]`: a missing key is a KeyError (500).
fn required<'a>(body: &'a Body, key: &str) -> ApiResult<&'a Value> {
    body.get(key)
        .ok_or_else(|| py_500(format!("KeyError: {key:?}")))
}

/// `POST /leads` (201): a warm lead assigned to a partner.
pub async fn create_lead(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let partner_id = required(&body, "partner_id")?;
    let customer_name = required(&body, "customer_name")?;
    let partner_id = col_uuid(Some(partner_id))?;
    let customer_name = col_str(Some(customer_name))?;
    let customer_email = col_str(body.get("customer_email"))?;
    let customer_company = col_str(body.get("customer_company"))?;
    let customer_country = col_str(body.get("customer_country"))?;
    let product_interest = match body.get("product_interest") {
        None => Some(Vec::new()),
        v => col_str_array(v)?,
    };
    let notes = col_str(body.get("notes"))?;
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_leads (id, partner_id, assigned_by_id, customer_name, customer_email, customer_company, \
           customer_country, product_interest, notes, status, assigned_at, is_deleted, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'new', $10, false, now(), now())",
    )
    .bind(id)
    .bind(partner_id)
    .bind(user.id)
    .bind(customer_name)
    .bind(customer_email)
    .bind(customer_company)
    .bind(customer_country)
    .bind(product_interest)
    .bind(notes)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "new"}),
    ))
}

/// `GET /leads`.
pub async fn list_leads(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<PartnerStatusPageQ>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT * FROM partner_leads WHERE partner_leads.is_deleted = false");
    if let Some(pid) = nonempty(&q.partner_id) {
        qb.push(" AND partner_leads.partner_id = ")
            .push_bind(pg_uuid(pid)?);
    }
    if let Some(s) = nonempty(&q.status) {
        qb.push(" AND partner_leads.status = ")
            .push_bind(s.to_string());
    }
    qb.push(" ORDER BY partner_leads.created_at DESC LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let rows = qb
        .build_query_as::<PartnerLead>()
        .fetch_all(&mut *conn)
        .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(lead_dict).collect(),
    )))
}

async fn live_lead(conn: &mut PgConnection, lead_id: &str) -> ApiResult<PartnerLead> {
    let id = pg_uuid(lead_id)?;
    sqlx::query_as::<_, PartnerLead>(
        "SELECT * FROM partner_leads WHERE partner_leads.id = $1 AND partner_leads.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("PartnerLead", Some(lead_id)))
}

/// `PATCH /leads/{lead_id}/status`.
pub async fn update_lead_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let lead = live_lead(&mut conn, &lead_id).await?;
    let status = match body.get("status") {
        Some(v) => col_str(Some(v))?,
        None => lead.status.clone(),
    };
    let converted = match get_truthy(&body, "converted_deal_id") {
        Some(v) => col_uuid(Some(v))?,
        None => lead.converted_deal_id,
    };
    sqlx::query(
        "UPDATE partner_leads SET status = $2, converted_deal_id = $3, updated_at = now() WHERE id = $1",
    )
    .bind(lead.id)
    .bind(&status)
    .bind(converted)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": lead.id.to_string(), "status": status}),
    ))
}

/// `DELETE /leads/{lead_id}` (204): soft delete.
pub async fn delete_lead(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(lead_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let lead = live_lead(&mut conn, &lead_id).await?;
    sqlx::query(
        "UPDATE partner_leads SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(lead.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── MDF (company admin) ──────────────────────────────────────────────────────

/// `_mdf_dict(m)`.
pub(crate) fn mdf_dict(m: &MDFRequest) -> Value {
    json!({
        "id": m.id.to_string(),
        "title": m.title,
        "description": m.description,
        "activity_type": m.activity_type,
        "requested_amount": dec_float(&m.requested_amount),
        // `if m.approved_amount`: a zero amount reads as None.
        "approved_amount": dec_float_opt(m.approved_amount.as_ref().filter(|a| !a.is_zero())),
        "currency": m.currency,
        "planned_date": date_iso_opt(m.planned_date.as_ref()),
        "status": m.status,
        "rejection_reason": m.rejection_reason,
        "created_at": iso(&m.created_at),
    })
}

/// `GET /mdf`.
pub async fn list_mdf(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<PartnerStatusPageQ>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT * FROM mdf_requests WHERE mdf_requests.is_deleted = false");
    if let Some(pid) = nonempty(&q.partner_id) {
        qb.push(" AND mdf_requests.partner_id = ")
            .push_bind(pg_uuid(pid)?);
    }
    if let Some(s) = nonempty(&q.status) {
        qb.push(" AND mdf_requests.status = ")
            .push_bind(s.to_string());
    }
    qb.push(" ORDER BY mdf_requests.created_at DESC LIMIT ")
        .push_bind(q.limit)
        .push(" OFFSET ")
        .push_bind(q.offset);
    let rows = qb
        .build_query_as::<MDFRequest>()
        .fetch_all(&mut *conn)
        .await?;
    // The review page shows the partner's name and reviews each request's claims: both ride on the list.
    let partner_ids: Vec<Uuid> = rows.iter().map(|m| m.partner_id).collect();
    let names: std::collections::HashMap<Uuid, Option<String>> =
        sqlx::query_as::<_, (Uuid, Option<String>)>(
            "SELECT partner_accounts.id, partner_accounts.company_name FROM partner_accounts \
         WHERE partner_accounts.id = ANY($1)",
        )
        .bind(&partner_ids)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect();
    let mdf_ids: Vec<Uuid> = rows.iter().map(|m| m.id).collect();
    let claims = sqlx::query_as::<_, MDFClaim>(
        "SELECT * FROM mdf_claims WHERE mdf_claims.mdf_request_id = ANY($1) ORDER BY mdf_claims.created_at DESC",
    )
    .bind(&mdf_ids)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|m| {
            let mut d = mdf_dict(m);
            d["partner_id"] = json!(m.partner_id.to_string());
            d["partner_name"] = json!(names.get(&m.partner_id).cloned().flatten());
            d["expected_outcome"] = json!(m.expected_outcome);
            d["claims"] = Value::Array(
                claims
                    .iter()
                    .filter(|c| c.mdf_request_id == m.id)
                    .map(mdf_claim_admin_dict)
                    .collect(),
            );
            d
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// A claim as the MDF review page reads it; the proof (an object key in the documents bucket) is served by
/// the authorised download route, never as a storage link.
fn mdf_claim_admin_dict(c: &MDFClaim) -> Value {
    let proof = c
        .proof_file_url
        .as_deref()
        .filter(|k| !k.is_empty())
        .map(|_| format!("{}/mdf/claims/{}/proof", super::PREFIX, c.id));
    json!({
        "id": c.id.to_string(),
        "mdf_request_id": c.mdf_request_id.to_string(),
        "claimed_amount": dec_float(&c.claimed_amount),
        "status": c.status,
        "proof_file_url": proof,
        "description": c.description,
        "created_at": iso(&c.created_at),
        "paid_at": iso_opt(c.paid_at.as_ref()),
        "payment_reference": c.payment_reference,
    })
}

/// `GET /mdf/claims/{claim_id}/proof`: a claim's proof file, to staff who review MDF (the same permission as
/// the MDF list), as a download with a safe content type.
pub async fn download_mdf_claim_proof(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(claim_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&claim_id)?;
    let key: Option<Option<String>> =
        sqlx::query_scalar("SELECT proof_file_url FROM mdf_claims WHERE mdf_claims.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    drop(conn);
    let Some(key) = key.flatten().filter(|k| !k.is_empty()) else {
        return Err(ApiError::not_found("MDFClaimProof", Some(&claim_id)));
    };
    // Only the keys the claim upload writes; anything else (an old external link) is not served from here.
    if !key.starts_with("mdf-claims/") || key.contains("..") {
        return Err(ApiError::http(404, "The file is not available."));
    }
    let bucket = granary_core::config::settings()
        .MINIO_BUCKET_DOCUMENTS
        .clone();
    let data = granary_services::storage::minio()
        .get_object(&bucket, &key)
        .await
        .map_err(|_| ApiError::http(404, "The file is not available."))?;
    let name = key.rsplit('/').next().unwrap_or("proof");
    // The stored name is "<8 hex>-<safe name>".
    let filename = name.split_once('-').map(|(_, n)| n).unwrap_or(name);
    let mime =
        granary_services::nary::files::guess_mime(filename).unwrap_or("application/octet-stream");
    Ok(crate::files::safe_file_response(
        data, mime, filename, false,
    ))
}

/// MDFRequest.status values (app/models/partner.py).
const MDF_REQUEST_STATUSES: &[&str] = &[
    "draft",
    "submitted",
    "under_review",
    "approved",
    "rejected",
    "completed",
    "cancelled",
];

fn mdf_claim_transitions(from: Option<&str>) -> &'static [&'static str] {
    match from {
        Some("submitted") => &["approved", "rejected"],
        Some("approved") => &["paid"],
        _ => &[],
    }
}

/// `_review_status(body, current)`: `{status}` wins, then `{action}` (approve | reject | paid, others as
/// given), else `current`. An unhashable action is a TypeError (500).
fn review_status(body: &Body, current: Option<&str>) -> ApiResult<Value> {
    if let Some(s) = get_truthy(body, "status") {
        return Ok(s.clone());
    }
    if let Some(a) = get_truthy(body, "action") {
        if !hashable(a) {
            return Err(py_500(format!("unhashable type: {}", py_repr(a))));
        }
        return Ok(match a.as_str() {
            Some("approve") => json!("approved"),
            Some("reject") => json!("rejected"),
            Some("paid") => json!("paid"),
            _ => a.clone(),
        });
    }
    Ok(current.map(|c| json!(c)).unwrap_or(Value::Null))
}

/// `D(x)` for a body value, as far as the route needs it: a finite Decimal, an infinity, NaN, or a value
/// `Decimal(str(x))` refuses (InvalidOperation).
enum PyNum {
    Fin(Decimal),
    PosInf,
    NegInf,
    NaN,
    Invalid,
}

fn py_d(v: &Value) -> PyNum {
    let text = match v {
        Value::Number(_) | Value::String(_) => py_str(v),
        // str(True) / str([..]) / str({..}) never parse as a Decimal.
        _ => return PyNum::Invalid,
    };
    match PyDec::parse(&text) {
        None => PyNum::Invalid,
        Some(PyDec::NonFinite) => {
            let t: String = text
                .trim()
                .chars()
                .filter(|c| *c != '_')
                .collect::<String>()
                .to_ascii_lowercase();
            if t.contains("nan") {
                PyNum::NaN
            } else if t.starts_with('-') {
                PyNum::NegInf
            } else {
                PyNum::PosInf
            }
        }
        Some(d) => match d.to_decimal() {
            Some(x) => PyNum::Fin(x),
            None => PyNum::Invalid,
        },
    }
}

/// `coalesce(sum(claimed_amount), 0)` of a request's claims that are not rejected.
pub(crate) async fn claimed_total(conn: &mut PgConnection, mdf_id: Uuid) -> ApiResult<Decimal> {
    let total: Decimal = sqlx::query_scalar(
        "SELECT coalesce(sum(mdf_claims.claimed_amount), 0) AS coalesce_1 FROM mdf_claims \
         WHERE mdf_claims.mdf_request_id = $1 AND mdf_claims.status != 'rejected'",
    )
    .bind(mdf_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(total)
}

/// `PATCH /mdf/{mdf_id}/review`.
pub async fn review_mdf(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(mdf_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&mdf_id)?;
    let mdf = sqlx::query_as::<_, MDFRequest>(
        "SELECT * FROM mdf_requests WHERE mdf_requests.id = $1 AND mdf_requests.is_deleted = false FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("MDFRequest", Some(&mdf_id)))?;
    let new_status = review_status(&body, mdf.status.as_deref())?;
    let Some(new_status) = new_status
        .as_str()
        .filter(|s| MDF_REQUEST_STATUSES.contains(s))
        .map(str::to_string)
    else {
        return Err(ApiError::http(
            400,
            format!("status must be one of: {}", MDF_REQUEST_STATUSES.join(", ")),
        ));
    };
    let given = body.get("approved_amount").filter(|v| !v.is_null());
    // approved_amount: the stored Decimal, or D(body value).
    let mut approved: Option<PyNum> = mdf.approved_amount.map(PyNum::Fin);
    if let Some(v) = given {
        match py_d(v) {
            PyNum::Invalid => {
                return Err(ApiError::http(400, "approved_amount must be a number"));
            }
            n => approved = Some(n),
        }
    }
    let cur = mdf.currency.as_deref().unwrap_or("None");
    if new_status == "approved" || given.is_some() {
        let requested = mdf.requested_amount;
        match &approved {
            None | Some(PyNum::NegInf) => {
                return Err(ApiError::http(
                    400,
                    "An approved MDF request needs an approved_amount above zero",
                ));
            }
            Some(PyNum::NaN) => return Err(py_500("InvalidOperation: comparison with NaN")),
            Some(PyNum::Fin(a)) if *a <= Decimal::ZERO => {
                return Err(ApiError::http(
                    400,
                    "An approved MDF request needs an approved_amount above zero",
                ));
            }
            _ => {}
        }
        let over = match &approved {
            Some(PyNum::PosInf) => true,
            Some(PyNum::Fin(a)) => *a > requested,
            _ => false,
        };
        if over {
            return Err(ApiError::http(
                400,
                format!(
                    "approved_amount can't exceed the requested {} {cur}",
                    resp::dec_str(&requested)
                ),
            ));
        }
        let claimed = claimed_total(&mut conn, mdf.id).await?;
        if let Some(PyNum::Fin(a)) = &approved {
            if *a < claimed {
                return Err(ApiError::http(
                    400,
                    format!(
                        "approved_amount can't be below the {} {cur} already claimed",
                        resp::dec_str(&claimed)
                    ),
                ));
            }
        }
    }
    let approved_value: Option<Decimal> = match approved {
        Some(PyNum::Fin(a)) => Some(a),
        None => None,
        // A non-finite amount that got here cannot be stored (asyncpg / Numeric(18, 2) refuse it).
        Some(_) => return Err(py_500("numeric field overflow")),
    };
    let rejection_reason = col_str(body.get("rejection_reason"))?;
    sqlx::query(
        "UPDATE mdf_requests SET status = $2, approved_amount = $3, rejection_reason = $4, reviewed_by_id = $5, \
           reviewed_at = $6, updated_at = now() WHERE id = $1",
    )
    .bind(mdf.id)
    .bind(&new_status)
    .bind(approved_value)
    .bind(rejection_reason)
    .bind(user.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": mdf.id.to_string(), "status": new_status}),
    ))
}

/// `PATCH /mdf/claims/{claim_id}/review`: approve / reject a submitted claim, pay an approved one.
pub async fn review_mdf_claim(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(claim_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&claim_id)?;
    let claim = sqlx::query_as::<_, MDFClaim>(
        "SELECT * FROM mdf_claims WHERE mdf_claims.id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("MDFClaim", Some(&claim_id)))?;
    let new_status = review_status(&body, None)?;
    let allowed = mdf_claim_transitions(claim.status.as_deref());
    let Some(new) = new_status.as_str().filter(|s| allowed.contains(s)) else {
        let mut detail = format!(
            "Cannot move a {} claim to {}",
            claim.status.as_deref().unwrap_or("None"),
            py_repr(&new_status)
        );
        if !allowed.is_empty() {
            detail.push_str(&format!(" (allowed: {})", allowed.join(", ")));
        }
        return Err(ApiError::http(400, detail));
    };
    let now: DateTime<Utc> = Utc::now();
    if new == "paid" {
        let paid_at = claim.paid_at.unwrap_or(now);
        let reference = match get_truthy(&body, "payment_reference") {
            Some(v) => col_str(Some(v))?,
            None => claim.payment_reference.clone(),
        };
        sqlx::query(
            "UPDATE mdf_claims SET status = $2, paid_at = $3, payment_reference = $4, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(new)
        .bind(paid_at)
        .bind(reference)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query(
            "UPDATE mdf_claims SET status = $2, reviewed_by_id = $3, reviewed_at = $4, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(new)
        .bind(user.id)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": id.to_string(), "status": new})))
}
