//! backend/app/routers/portal_auth.py, mounted at `/api/v1/portal` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (public, portal or WebSocket routes).
//!
//! Portal self-registration and admin approval: vendors and partners register themselves; a company
//! admin reviews and approves (creating the account and emailing the credentials).

use crate::extract::pg_uuid;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::crypto::tokens::{new_temp_password, OsRandom};
use granary_compat::pyfmt::html_escape;
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{v, EmailStr, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use super::auth::{hash_password, internal};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/portal";

/// The routes of `router` in portal_auth.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/register/vendor"), post(register_vendor))
        .route(&at(PREFIX, "/register/partner"), post(register_partner))
        .route(
            &at(PREFIX, "/register/partner-types"),
            get(partner_registration_types),
        )
        .route(&at(PREFIX, "/registrations"), get(list_registrations))
        .route(
            &at(PREFIX, "/registrations/{reg_id}/approve"),
            post(approve_registration),
        )
        .route(
            &at(PREFIX, "/registrations/{reg_id}/reject"),
            post(reject_registration),
        )
}

granary_core::py_model! {
    /// `class VendorRegistrationRequest(BaseModel)`.
    /// Lengths capped at the columns' sizes (notes: 5000): anyone may post it.
    pub struct VendorRegistrationRequest {
        pub company_name: String => v::str_c(v::StrC::new().max_length(255)),
        pub contact_name: String => v::str_c(v::StrC::new().max_length(255)),
        pub email: EmailStr,
        pub phone: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(50))),
        pub vendor_type: String = "professional_services" => v::str_c(v::StrC::new().max_length(50)),
        pub country: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(100))),
        pub website: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(500))),
        pub notes: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(5000))),
    }
}

granary_core::py_model! {
    /// `class PartnerRegistrationRequest(BaseModel)`.
    /// Lengths capped at the columns' sizes (notes: 5000): anyone may post it.
    pub struct PartnerRegistrationRequest {
        pub company_name: String => v::str_c(v::StrC::new().max_length(255)),
        pub contact_name: String => v::str_c(v::StrC::new().max_length(255)),
        pub email: EmailStr,
        pub phone: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(50))),
        pub partner_type: String = "reseller" => v::str_c(v::StrC::new().max_length(50)),
        pub country: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(100))),
        pub website: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(500))),
        pub notes: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(5000))),
    }
}

granary_core::py_model! {
    /// The query parameters of `list_registrations`.
    pub struct ListRegistrationsQ {
        pub status: Option<String> = Some("pending".to_string()),
        pub portal_type: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

const SUBMITTED: &str =
    "Registration submitted. You will receive credentials by email once approved.";

#[allow(clippy::too_many_arguments)]
async fn insert_registration(
    db: &mut PgConnection,
    portal_type: &str,
    company_name: &str,
    contact_name: &str,
    email: &str,
    phone: Option<&str>,
    vendor_type: Option<&str>,
    partner_type: Option<&str>,
    country: Option<&str>,
    website: Option<&str>,
    notes: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO portal_registration_requests (portal_type, status, company_name, contact_name, email, \
         phone, vendor_type, partner_type, country, website, notes, id) \
         VALUES ($1, 'pending', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(portal_type)
    .bind(company_name)
    .bind(contact_name)
    .bind(email)
    .bind(phone)
    .bind(vendor_type)
    .bind(partner_type)
    .bind(country)
    .bind(website)
    .bind(notes)
    .bind(Uuid::new_v4())
    .execute(db)
    .await?;
    Ok(())
}

/// `POST /register/vendor` (201, public).
pub async fn register_vendor(
    db: Db,
    ip: crate::extract::ClientIp,
    PyJson(body): PyJson<VendorRegistrationRequest>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    crate::public_limit::check(&mut conn, "register", &ip).await?;
    insert_registration(
        &mut conn,
        "vendor",
        &body.company_name,
        &body.contact_name,
        &pystr::lower(body.email.as_str()),
        body.phone.as_deref(),
        Some(&body.vendor_type),
        None,
        body.country.as_deref(),
        body.website.as_deref(),
        body.notes.as_deref(),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"message": SUBMITTED})))
}

/// `GET /register/partner-types` (public): the partner types the registration form offers (the same list
/// staff screens use).
pub async fn partner_registration_types() -> Response {
    resp::json(super::partner::options_json(super::partner::PARTNER_TYPES))
}

/// `POST /register/partner` (201, public).
pub async fn register_partner(
    db: Db,
    ip: crate::extract::ClientIp,
    PyJson(body): PyJson<PartnerRegistrationRequest>,
) -> ApiResult<Response> {
    super::partner::require_option(
        super::partner::PARTNER_TYPES,
        "partner_type",
        &body.partner_type,
    )?;
    let mut conn = db.conn().await?;
    crate::public_limit::check(&mut conn, "register", &ip).await?;
    insert_registration(
        &mut conn,
        "partner",
        &body.company_name,
        &body.contact_name,
        &pystr::lower(body.email.as_str()),
        body.phone.as_deref(),
        None,
        Some(&body.partner_type),
        body.country.as_deref(),
        body.website.as_deref(),
        body.notes.as_deref(),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"message": SUBMITTED})))
}

fn require_portal_admin(u: &AuthUser) -> ApiResult<()> {
    if !u.is_superadmin && !u.perms.contains("system:admin") {
        return Err(ApiError::http(403, "Admin access required"));
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RegRow {
    id: Uuid,
    portal_type: String,
    status: String,
    company_name: String,
    contact_name: String,
    email: String,
    phone: Option<String>,
    vendor_type: Option<String>,
    partner_type: Option<String>,
    country: Option<String>,
    website: Option<String>,
    notes: Option<String>,
    rejection_reason: Option<String>,
    reviewed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const REG_COLUMNS: &str =
    "id, portal_type, status, company_name, contact_name, email, phone, vendor_type, \
     partner_type, country, website, notes, rejection_reason, reviewed_at, created_at";

/// `_reg_dict(r)`.
fn reg_dict(r: &RegRow) -> Value {
    json!({
        "id": r.id.to_string(),
        "portal_type": r.portal_type,
        "status": r.status,
        "company_name": r.company_name,
        "contact_name": r.contact_name,
        "email": r.email,
        "phone": r.phone,
        "vendor_type": r.vendor_type,
        "partner_type": r.partner_type,
        "country": r.country,
        "website": r.website,
        "notes": r.notes,
        "rejection_reason": r.rejection_reason,
        "reviewed_at": resp::iso_opt(r.reviewed_at.as_ref()),
        "created_at": resp::iso(&r.created_at),
    })
}

/// `GET /registrations`: internal admin list.
pub async fn list_registrations(
    CurrentUser(me): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListRegistrationsQ>,
) -> ApiResult<Response> {
    require_portal_admin(&me)?;
    let mut sql = format!("SELECT {REG_COLUMNS} FROM portal_registration_requests");
    let mut conds: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if let Some(s) = q.status.as_deref().filter(|s| !s.is_empty()) {
        args.push(s.to_string());
        conds.push(format!(
            "portal_registration_requests.status = ${}",
            args.len()
        ));
    }
    if let Some(p) = q.portal_type.as_deref().filter(|s| !s.is_empty()) {
        args.push(p.to_string());
        conds.push(format!(
            "portal_registration_requests.portal_type = ${}",
            args.len()
        ));
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    sql.push_str(&format!(
        " ORDER BY portal_registration_requests.created_at DESC LIMIT ${} OFFSET ${}",
        args.len() + 1,
        args.len() + 2
    ));
    let mut query = sqlx::query_as::<_, RegRow>(&sql);
    for a in &args {
        query = query.bind(a);
    }
    let mut conn = db.conn().await?;
    let rows = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(reg_dict).collect(),
    )))
}

async fn load_reg(db: &mut PgConnection, reg_id: &str) -> ApiResult<RegRow> {
    let id = pg_uuid(reg_id)?;
    let reg = sqlx::query_as::<_, RegRow>(&format!(
        "SELECT {REG_COLUMNS} FROM portal_registration_requests WHERE portal_registration_requests.id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(reg) = reg else {
        return Err(ApiError::http(404, "Registration not found"));
    };
    if reg.status != "pending" {
        return Err(ApiError::http(
            400,
            format!("Registration is already {}", reg.status),
        ));
    }
    Ok(reg)
}

async fn assign_role_by_slug(db: &mut PgConnection, user_id: Uuid, slug: &str) -> ApiResult<()> {
    let role: Option<(Uuid,)> = sqlx::query_as("SELECT roles.id FROM roles WHERE roles.slug = $1")
        .bind(slug)
        .fetch_optional(&mut *db)
        .await?;
    if let Some((role_id,)) = role {
        sqlx::query(
            "INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(role_id)
        .execute(db)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_portal_user(
    db: &mut PgConnection,
    name: &str,
    email: &str,
    password_hash: &str,
    portal_type: &str,
    vendor_id: Option<Uuid>,
    partner_account_id: Option<Uuid>,
) -> ApiResult<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (email, name, password_hash, email_signature_enabled, is_active, is_superadmin, \
         is_system_admin, portal_type, partner_account_id, vendor_id, must_change_password, id) \
         VALUES ($1, $2, $3, true, true, false, false, $4, $5, $6, true, $7)",
    )
    .bind(email)
    .bind(name)
    .bind(password_hash)
    .bind(portal_type)
    .bind(partner_account_id)
    .bind(vendor_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(id)
}

/// `POST /registrations/{reg_id}/approve`: creates the vendor/partner and its user, then emails the
/// credentials and reports whether that worked.
pub async fn approve_registration(
    State(state): State<AppState>,
    Path(reg_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    require_portal_admin(&me)?;
    let mut conn = db.conn().await?;
    let mut reg = load_reg(&mut conn, &reg_id).await?;

    // Case-insensitive: login matches emails that way, so "A@x.com" and "a@x.com" are one account.
    reg.email = pystr::lower(pystr::strip(&reg.email));
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT users.id FROM users WHERE lower(users.email) = $1 LIMIT 1")
            .bind(&reg.email)
            .fetch_optional(&mut *conn)
            .await?;
    if existing.is_some() {
        return Err(ApiError::http(409, "A user with this email already exists"));
    }

    let temp_password = new_temp_password(&mut OsRandom);
    let pw_hash = hash_password(&temp_password)?;

    let user_id = if reg.portal_type == "vendor" {
        let vendor_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO vendors (name, vendor_type, status, withholding_applicable, contact_name, contact_email, \
             contact_phone, country, has_portal_access, total_spend, spend_currency, id, is_deleted) \
             VALUES ($1, $2, 'onboarding', false, $3, $4, $5, $6, true, 0, coalesce((SELECT value FROM system_settings WHERE key = 'currency.functional'), 'USD'), $7, false)",
        )
        .bind(&reg.company_name)
        .bind(
            reg.vendor_type
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("professional_services"),
        )
        .bind(&reg.contact_name)
        .bind(&reg.email)
        .bind(&reg.phone)
        .bind(&reg.country)
        .bind(vendor_id)
        .execute(&mut *conn)
        .await?;
        let uid = insert_portal_user(
            &mut conn,
            &reg.contact_name,
            &reg.email,
            &pw_hash,
            "vendor",
            Some(vendor_id),
            None,
        )
        .await?;
        assign_role_by_slug(&mut conn, uid, "vendor_user").await?;
        uid
    } else {
        let partner_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO partner_accounts (company_name, contact_name, contact_email, contact_phone, country, \
             website, partner_type, tier, status, territory_countries, territory_verticals, \
             authorized_products, authorized_services, is_exclusive, commitment_currency, \
             certified_staff_count, certification_details, cumulative_units, cumulative_revenue, \
             current_year_units, current_year_revenue, id, is_deleted) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'authorized', 'pending_agreement', '{}', '{}', '{}', '{}', \
             false, 'USD', 0, '[]'::jsonb, 0, 0, 0, 0, $8, false)",
        )
        .bind(&reg.company_name)
        .bind(&reg.contact_name)
        .bind(&reg.email)
        .bind(&reg.phone)
        .bind(&reg.country)
        .bind(&reg.website)
        .bind(
            reg.partner_type
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("reseller"),
        )
        .bind(partner_id)
        .execute(&mut *conn)
        .await?;
        let uid = insert_portal_user(
            &mut conn,
            &reg.contact_name,
            &reg.email,
            &pw_hash,
            "partner",
            None,
            Some(partner_id),
        )
        .await?;
        assign_role_by_slug(&mut conn, uid, "partner_user").await?;
        uid
    };

    sqlx::query(
        "UPDATE portal_registration_requests SET status = 'approved', email = $2, reviewed_by_id = $3, \
         reviewed_at = $4, created_user_id = $5, updated_at = now() WHERE portal_registration_requests.id = $1",
    )
    .bind(reg.id)
    .bind(&reg.email)
    .bind(me.id)
    .bind(Utc::now())
    .bind(user_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;

    // Send the credentials now and report the outcome.
    let (sent, problem) = {
        let mut conn = db.conn().await?;
        send_welcome_email(&state, &mut conn, &reg, &temp_password).await
    };
    let message = if sent {
        "Registration approved. Credentials sent to applicant.".to_string()
    } else {
        format!(
            "Registration approved, but the welcome email was NOT sent ({}). \
             Reset the user's password from Users to issue new credentials.",
            problem.clone().unwrap_or_default()
        )
    };
    Ok(resp::json(json!({
        "message": message,
        "user_id": user_id.to_string(),
        "email_sent": sent,
        "email_error": if sent { None } else { problem },
    })))
}

/// `POST /registrations/{reg_id}/reject`.
pub async fn reject_registration(
    Path(reg_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    require_portal_admin(&me)?;
    let mut conn = db.conn().await?;
    let reg = load_reg(&mut conn, &reg_id).await?;
    let reason = match body.get("rejection_reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        // asyncpg refuses a non-str value for a Text column (a 500).
        Some(other) => {
            return Err(internal(format!(
                "invalid input for rejection_reason: {other}"
            )))
        }
    };
    sqlx::query(
        "UPDATE portal_registration_requests SET status = 'rejected', reviewed_by_id = $2, reviewed_at = $3, \
         rejection_reason = $4, updated_at = now() WHERE portal_registration_requests.id = $1",
    )
    .bind(reg.id)
    .bind(me.id)
    .bind(Utc::now())
    .bind(&reason)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"message": "Registration rejected."})))
}

/// `str.capitalize()`.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + &pystr::lower(chars.as_str()),
        None => String::new(),
    }
}

/// `_send_welcome_email(db, reg, temp_password)`: (sent, reason it wasn't).
async fn send_welcome_email(
    state: &AppState,
    db: &mut PgConnection,
    reg: &RegRow,
    temp_password: &str,
) -> (bool, Option<String>) {
    use granary_services::email::{_send_via_config, load_system_smtp};
    let settings = &state.settings;
    let result: Result<(bool, Option<String>), String> = async {
        let cfg = load_system_smtp(db).await.map_err(|e| e.to_string())?;
        if cfg.host.is_empty() || cfg.from_email.is_empty() {
            return Ok((false, Some("outgoing email is not configured".to_string())));
        }
        let portal = capitalize(&reg.portal_type);
        let login_url = format!("{}/{}/login", settings.APP_URL, reg.portal_type);
        // settings.APP_NAME.split()[0]: an empty name raises IndexError.
        let brand = pystr::split(&settings.APP_NAME)
            .first()
            .map(|s| s.to_string())
            .ok_or_else(|| "list index out of range".to_string())?;
        let e = |s: &str| html_escape(s, true);
        // Names and the company come from a public registration form: escape them in the HTML.
        let html = format!(
            r#"
            <html><body style="font-family:Arial,sans-serif;color:#111;max-width:560px;margin:40px auto;padding:0 20px">
              <div style="border-bottom:3px solid #1f4e79;padding-bottom:16px;margin-bottom:24px">
                <span style="font-size:24px;font-weight:900;color:#1f4e79;letter-spacing:4px">{brand}</span>
              </div>
              <h2>Welcome to the {portal_e} Portal</h2>
              <p>Hello <strong>{contact}</strong>,</p>
              <p>Your {portal_lower} portal registration for <strong>{company}</strong> has been approved.</p>
              <p>Your login credentials:</p>
              <div style="background:#f5f5f5;border-radius:8px;padding:16px;margin:16px 0">
                <p style="margin:0 0 8px"><strong>Email:</strong> {email}</p>
                <p style="margin:0"><strong>Temporary Password:</strong> <code style="font-size:15px">{password}</code></p>
              </div>
              <p>You will be prompted to change your password on first login.</p>
              <div style="margin:32px 0;text-align:center">
                <a href="{login_url}" style="background:#1f4e79;color:white;padding:14px 32px;border-radius:10px;font-weight:700;font-size:15px;text-decoration:none">
                  Log In to {portal_e} Portal
                </a>
              </div>
            </body></html>
            "#,
            brand = e(&brand),
            portal_e = e(&portal),
            contact = e(&reg.contact_name),
            portal_lower = e(&pystr::lower(&portal)),
            company = e(&reg.company_name),
            email = e(&reg.email),
            password = e(temp_password),
            login_url = e(&login_url),
        );
        let subject = format!("Your {portal} Portal Access - {}", settings.APP_NAME);
        let ok = _send_via_config(
            &cfg,
            std::slice::from_ref(&reg.email),
            &subject,
            &html,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(if ok {
            (true, None)
        } else {
            (
                false,
                Some("the mail server rejected or did not accept the message".to_string()),
            )
        })
    }
    .await;
    match result {
        Ok(v) => v,
        Err(exc) => {
            tracing::error!("portal welcome email failed for {}: {}", reg.email, exc);
            (false, Some("sending failed".to_string()))
        }
    }
}
