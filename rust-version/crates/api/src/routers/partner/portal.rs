//! partner.py, the partner portal (`/portal/...`): only users with portal_type "partner" and a partner
//! account get past `_require_partner_portal`; every query is scoped to that account.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::{Datelike, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::money::D;
use granary_core::validation::{v, Cx, Loc, PyJson, PyObj, Validate};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::partner::{
    MDFRequest, PartnerAccount, PartnerCommission, PartnerDealRegistration, PartnerLead,
    PartnerResource,
};

use super::admin::{claimed_total, insert_registration, lead_dict, mdf_dict, MONEY_MAX};
use super::{fromisoformat_or_422, py_500, require_partner_portal, tier_rank};
use crate::extract::PyForm;
use crate::resp::{self, date_iso_opt, dec_float, dec_float_opt, iso, iso_opt};

granary_core::py_model! {
    /// `class PortalDealCreate(BaseModel)`.
    pub struct PortalDealCreate {
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
    /// `class MDFRequestCreate(BaseModel)`.
    pub struct MDFRequestCreate {
        pub title: String,
        pub description: Option<String> = None,
        pub activity_type: String = "event",
        pub requested_amount: Decimal => v::decimal_c(v::DecC::new().gt("0").le(MONEY_MAX)),
        pub currency: String = "USD",
        pub planned_date: Option<String> = None,
        pub planned_location: Option<String> = None,
        pub expected_outcome: Option<String> = None,
    }
}

/// `_portal_partner(current_user, db)`: the caller's live partner account (403 when removed).
pub(crate) async fn portal_partner(
    conn: &mut PgConnection,
    user: &granary_core::auth::AuthUser,
) -> ApiResult<PartnerAccount> {
    let partner_id = require_partner_portal(user)?;
    sqlx::query_as::<_, PartnerAccount>(
        "SELECT * FROM partner_accounts WHERE partner_accounts.id = $1 AND partner_accounts.is_deleted = false",
    )
    .bind(partner_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(403, "Partner account not found or removed"))
}

/// `GET /portal/account`: the calling partner's account profile.
pub async fn portal_get_account(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let p = portal_partner(&mut conn, &user).await?;
    let mut am = Value::Null;
    if let Some(am_id) = p.assigned_am_id {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT name, email FROM users WHERE users.id = $1")
                .bind(am_id)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some((name, email)) = row {
            am = json!({"name": name, "email": email});
        }
    }
    Ok(resp::json(json!({
        "id": p.id.to_string(), "company_name": p.company_name,
        "partner_type": p.partner_type, "tier": p.tier, "status": p.status,
        "country": p.country, "website": p.website,
        "contact_name": p.contact_name, "contact_email": p.contact_email,
        "contact_phone": p.contact_phone,
        "agreement_ref": p.agreement_ref,
        "agreement_expires_at": date_iso_opt(p.agreement_expires_at.as_ref()),
        "territory_countries": p.territory_countries.clone().unwrap_or_default(),
        "authorized_products": p.authorized_products.clone().unwrap_or_default(),
        "authorized_services": p.authorized_services.clone().unwrap_or_default(),
        "account_manager": am,
    })))
}

/// Whether a partner may see a resource: its own tier and lower tiers (a resource's `min_tier` ranks at
/// or below the partner's), and either for every partner type or for this partner's type.
pub(crate) fn resource_visible_to(r: &PartnerResource, p: &PartnerAccount) -> bool {
    tier_rank(r.min_tier.as_deref()) <= tier_rank(p.tier.as_deref())
        && match r.partner_types.as_deref() {
            None | Some([]) => true,
            Some(types) => types.contains(&p.partner_type),
        }
}

/// `GET /portal/resources`: active resources this partner's tier and type may access.
pub async fn portal_list_resources(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let p = portal_partner(&mut conn, &user).await?;
    let rows = sqlx::query_as::<_, PartnerResource>(
        "SELECT * FROM partner_resources WHERE partner_resources.is_active = true \
         ORDER BY partner_resources.resource_type, partner_resources.title",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .filter(|r| resource_visible_to(r, &p))
        .map(|r| {
            json!({
                "id": r.id.to_string(), "title": r.title, "description": r.description,
                "resource_type": r.resource_type, "file_url": r.file_url,
                "thumbnail_url": r.thumbnail_url, "version": r.version,
                "tier_required": r.min_tier,
                "created_at": iso(&r.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /portal/deals/stats`.
pub async fn portal_deal_stats(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT partner_deal_registrations.status, count(*) AS count_1 FROM partner_deal_registrations \
         WHERE partner_deal_registrations.partner_id = $1 GROUP BY partner_deal_registrations.status",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let by_stage: Vec<Value> = rows
        .into_iter()
        .map(|(status, count)| json!({"stage": status, "count": count}))
        .collect();
    Ok(resp::json(json!({ "by_stage": by_stage })))
}

/// `_by_currency(pairs)`: sum (currency, amount) pairs per currency, `{currency: float}` in first-seen
/// order; commissions in different currencies are never added together.
fn by_currency<'a>(
    pairs: impl Iterator<Item = (Option<&'a str>, Option<Decimal>)>,
) -> Map<String, Value> {
    let mut totals: Vec<(String, Decimal)> = Vec::new();
    for (currency, amount) in pairs {
        let Some(amount) = amount else { continue };
        let code = match currency {
            Some(c) if !c.is_empty() => c.to_uppercase(),
            _ => "USD".to_string(),
        };
        match totals.iter_mut().find(|(c, _)| *c == code) {
            Some((_, t)) => *t += D(amount),
            None => totals.push((code, D(0i64) + D(amount))),
        }
    }
    totals
        .into_iter()
        .map(|(c, t)| (c, dec_float(&t)))
        .collect()
}

/// `_sole_currency(maps)`: the one currency every map is in, or None.
fn sole_currency(maps: &[&Map<String, Value>]) -> Value {
    let mut codes: Vec<&String> = Vec::new();
    for m in maps {
        for k in m.keys() {
            if !codes.contains(&k) {
                codes.push(k);
            }
        }
    }
    if codes.len() == 1 {
        Value::String(codes[0].clone())
    } else {
        Value::Null
    }
}

/// `GET /portal/commissions/stats`.
pub async fn portal_commission_stats(
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let commissions = sqlx::query_as::<_, PartnerCommission>(
        "SELECT * FROM partner_commissions WHERE partner_commissions.partner_id = $1 \
         ORDER BY partner_commissions.created_at DESC",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let year = Utc::now().year();
    let ytd = by_currency(
        commissions
            .iter()
            .filter(|c| {
                c.status.as_deref() == Some("paid") && c.paid_at.is_some_and(|p| p.year() == year)
            })
            .map(|c| (c.currency.as_deref(), Some(c.amount))),
    );
    let pending = by_currency(
        commissions
            .iter()
            .filter(|c| matches!(c.status.as_deref(), Some("pending") | Some("approved")))
            .map(|c| (c.currency.as_deref(), Some(c.amount))),
    );
    let currency = sole_currency(&[&ytd, &pending]);
    Ok(resp::json(
        json!({"ytd": ytd, "pending": pending, "currency": currency}),
    ))
}

/// `GET /portal/leads/stats`.
pub async fn portal_lead_stats(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT partner_leads.status, count(*) AS count_1 FROM partner_leads \
         WHERE partner_leads.partner_id = $1 AND partner_leads.is_deleted = false GROUP BY partner_leads.status",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let new = rows
        .iter()
        .find(|(s, _)| s.as_deref() == Some("new"))
        .map(|(_, n)| *n)
        .unwrap_or(0);
    let total: i64 = rows.iter().map(|(_, n)| *n).sum();
    Ok(resp::json(json!({"new": new, "total": total})))
}

/// `GET /portal/mdf/stats`.
pub async fn portal_mdf_stats(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let requests = sqlx::query_as::<_, MDFRequest>(
        "SELECT * FROM mdf_requests WHERE mdf_requests.partner_id = $1 AND mdf_requests.is_deleted = false \
         ORDER BY mdf_requests.created_at DESC",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let claimed: Vec<(Option<String>, Decimal)> = sqlx::query_as(
        "SELECT mdf_requests.currency, coalesce(sum(mdf_claims.claimed_amount), 0) AS coalesce_1 \
         FROM mdf_claims JOIN mdf_requests ON mdf_requests.id = mdf_claims.mdf_request_id \
         WHERE mdf_requests.partner_id = $1 AND mdf_requests.is_deleted = false \
           AND mdf_claims.status != 'rejected' GROUP BY mdf_requests.currency",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let approved = by_currency(
        requests
            .iter()
            .filter(|r| r.approved_amount.is_some())
            .map(|r| (r.currency.as_deref(), r.approved_amount)),
    );
    let claimed = by_currency(claimed.iter().map(|(c, a)| (c.as_deref(), Some(*a))));
    let currency = sole_currency(&[&approved, &claimed]);
    Ok(resp::json(
        json!({"approved": approved, "claimed": claimed, "currency": currency}),
    ))
}

/// `GET /portal/mdf/claims`: claims filed against the calling partner's MDF requests.
pub async fn portal_list_mdf_claims(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query(
        "SELECT mdf_claims.*, mdf_requests.title AS mdf_request_title FROM mdf_claims \
         JOIN mdf_requests ON mdf_requests.id = mdf_claims.mdf_request_id \
         WHERE mdf_requests.partner_id = $1 AND mdf_requests.is_deleted = false \
         ORDER BY mdf_claims.created_at DESC",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let c = <granary_db::rows::partner::MDFClaim as sqlx::FromRow<_>>::from_row(row)?;
        let title: String = row.try_get("mdf_request_title")?;
        out.push(json!({
            "id": c.id.to_string(), "mdf_request_id": c.mdf_request_id.to_string(), "mdf_request_title": title,
            "claimed_amount": dec_float(&c.claimed_amount), "description": c.description,
            "status": c.status,
            "created_at": iso(&c.created_at),
            "paid_at": iso_opt(c.paid_at.as_ref()),
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `GET /portal/deals`: the calling partner's deal registrations.
pub async fn portal_list_deals(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let regs = sqlx::query_as::<_, PartnerDealRegistration>(
        "SELECT * FROM partner_deal_registrations WHERE partner_deal_registrations.partner_id = $1 \
         ORDER BY partner_deal_registrations.created_at DESC",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = regs
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(),
                "title": r.title,
                "customer_org_name": r.customer_org_name,
                "customer_contact_name": r.customer_contact_name,
                "customer_contact_email": r.customer_contact_email,
                // `if r.estimated_value`: a zero value reads as None.
                "estimated_value": dec_float_opt(r.estimated_value.as_ref().filter(|v| !v.is_zero())),
                "currency": r.currency,
                "expected_close_date": date_iso_opt(r.expected_close_date.as_ref()),
                "status": r.status,
                // The partner sees why its own registration was turned down (never the internal notes).
                "rejection_reason": if r.status.as_deref() == Some("rejected") {
                    r.rejection_reason.clone()
                } else {
                    None
                },
                "created_at": iso(&r.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /portal/deals` (201): register a deal on behalf of the calling partner.
pub async fn portal_create_deal(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PortalDealCreate>,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
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

/// `GET /portal/commissions`: the calling partner's commissions.
pub async fn portal_list_commissions(
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query_as::<_, PartnerCommission>(
        "SELECT * FROM partner_commissions WHERE partner_commissions.partner_id = $1 \
         ORDER BY partner_commissions.created_at DESC",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(),
                "commission_type": c.commission_type,
                "base_amount": dec_float(&c.base_amount),
                "rate": dec_float(&c.rate),
                "amount": dec_float(&c.amount),
                "currency": c.currency,
                "status": c.status,
                "period_month": c.period_month,
                "period_year": c.period_year,
                "deal_registration_id": c.deal_registration_id.map(|d| d.to_string()),
                "paid_at": iso_opt(c.paid_at.as_ref()),
                "created_at": iso(&c.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /portal/leads`.
pub async fn portal_list_leads(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query_as::<_, PartnerLead>(
        "SELECT * FROM partner_leads WHERE partner_leads.partner_id = $1 AND partner_leads.is_deleted = false \
         ORDER BY partner_leads.created_at DESC",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(lead_dict).collect(),
    )))
}

/// `POST /portal/mdf` (201).
pub async fn portal_create_mdf(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<MDFRequestCreate>,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let planned = match body.planned_date.as_deref() {
        Some(s) if !s.is_empty() => Some(fromisoformat_or_422(s)?),
        _ => None,
    };
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mdf_requests (id, partner_id, submitted_by_id, title, description, activity_type, \
           requested_amount, currency, planned_date, planned_location, expected_outcome, status, is_deleted, \
           created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'submitted', false, now(), now())",
    )
    .bind(id)
    .bind(partner_id)
    .bind(user.id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(&body.activity_type)
    .bind(body.requested_amount)
    .bind(&body.currency)
    .bind(planned)
    .bind(&body.planned_location)
    .bind(&body.expected_outcome)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "submitted"}),
    ))
}

/// `GET /portal/mdf`.
pub async fn portal_list_mdf(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query_as::<_, MDFRequest>(
        "SELECT * FROM mdf_requests WHERE mdf_requests.partner_id = $1 AND mdf_requests.is_deleted = false \
         ORDER BY mdf_requests.created_at DESC",
    )
    .bind(partner_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(mdf_dict).collect(),
    )))
}

/// `_safe_filename(name)`: basename only, restricted to letters, digits, dot, dash and underscore.
pub(crate) fn safe_filename(name: Option<&str>) -> String {
    let n = name.unwrap_or("").replace('\\', "/");
    let base = n.rsplit('/').next().unwrap_or("");
    // re.sub(r"[^A-Za-z0-9._-]+", "_", base)
    let mut out = String::new();
    let mut in_run = false;
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '.' || c == '_');
    let cut: String = trimmed.chars().take(120).collect();
    if cut.is_empty() {
        "proof".to_string()
    } else {
        cut
    }
}

/// `POST /portal/mdf/{mdf_id}/claims` (201, form data): a claim against an approved MDF request, within the
/// approved amount; an optional proof file goes to MinIO.
pub async fn portal_submit_mdf_claim(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(mdf_id): Path<String>,
    form: PyForm,
) -> ApiResult<Response> {
    // Form validation (FastAPI, before the handler): claimed_amount: Decimal = Form(...), description:
    // Optional[str] = Form(None), proof_file: Optional[UploadFile] = File(None).
    let mut cx = Cx::new();
    let claimed_raw = form.req_text(&mut cx, "claimed_amount");
    let claimed_amount: Option<Decimal> = claimed_raw.and_then(|s| {
        cx.at(Loc::Key("body".into()), |cx| {
            cx.at_key("claimed_amount", |cx| {
                <Decimal as Validate>::validate(&PyObj::str(s), cx)
            })
        })
    });
    let description = form.opt_text("description");
    let proof_file = form.opt_file(&mut cx, "proof_file");
    form.check(cx)?;
    let claimed_amount = claimed_amount.expect("validated");

    let partner_id = require_partner_portal(&user)?;
    if claimed_amount <= Decimal::ZERO {
        return Err(ApiError::http(
            400,
            "claimed_amount must be greater than zero",
        ));
    }
    let mut conn = db.conn().await?;
    let id = pg_uuid(&mdf_id)?;
    // Lock the request: concurrent claims against it are checked one at a time.
    let mdf = sqlx::query_as::<_, MDFRequest>(
        "SELECT * FROM mdf_requests WHERE mdf_requests.id = $1 AND mdf_requests.partner_id = $2 \
           AND mdf_requests.status = 'approved' AND mdf_requests.is_deleted = false FOR UPDATE",
    )
    .bind(id)
    .bind(partner_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Approved MDF request not found"))?;
    let already = D(claimed_total(&mut conn, mdf.id).await?);
    let approved = D(mdf.approved_amount);
    let cur = mdf.currency.as_deref().unwrap_or("None");
    if already + claimed_amount > approved {
        return Err(ApiError::http(
            400,
            format!(
                "Claims would total {} {cur}, above the approved {} {cur} ({} {cur} left to claim)",
                resp::dec_str(&(already + claimed_amount)),
                resp::dec_str(&approved),
                resp::dec_str(&(approved - already)),
            ),
        ));
    }
    let mut proof_url: Option<String> = None;
    if let Some(file) = proof_file {
        let settings = granary_core::config::settings();
        let store = granary_services::storage::minio();
        let bucket = settings.MINIO_BUCKET_DOCUMENTS.clone();
        granary_services::storage::ensure_bucket(store.as_ref(), &bucket)
            .await
            .map_err(py_500)?;
        let object_name = format!(
            "mdf-claims/{}/{}-{}",
            mdf.id,
            granary_compat::crypto::tokens::token_hex(
                &mut granary_compat::crypto::tokens::OsRandom,
                4
            ),
            safe_filename(Some(&file.filename))
        );
        store
            .put_object(
                &bucket,
                &object_name,
                &file.data,
                file.content_type
                    .as_deref()
                    .filter(|c| !c.is_empty())
                    .unwrap_or("application/octet-stream"),
            )
            .await
            .map_err(py_500)?;
        proof_url = Some(object_name);
    }
    let claim_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mdf_claims (id, mdf_request_id, submitted_by_id, claimed_amount, description, proof_file_url, \
           status, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, 'submitted', now(), now())",
    )
    .bind(claim_id)
    .bind(mdf.id)
    .bind(user.id)
    .bind(claimed_amount)
    .bind(&description)
    .bind(&proof_url)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    // db.refresh(claim): the stored (rounded) amount and the server timestamps.
    let mut conn = db.conn().await?;
    let row = sqlx::query(
        "SELECT claimed_amount, description, status, created_at FROM mdf_claims WHERE mdf_claims.id = $1",
    )
    .bind(claim_id)
    .fetch_one(&mut *conn)
    .await?;
    let stored: Decimal = row.try_get("claimed_amount")?;
    let created_at: chrono::DateTime<Utc> = row.try_get("created_at")?;
    let description: Option<String> = row.try_get("description")?;
    let status: Option<String> = row.try_get("status")?;
    Ok(resp::json_status(
        201,
        json!({
            "id": claim_id.to_string(), "mdf_request_id": mdf.id.to_string(), "mdf_request_title": mdf.title,
            "claimed_amount": dec_float(&stored), "description": description,
            "status": status,
            "created_at": iso(&created_at),
            "paid_at": null,
        }),
    ))
}
