//! backend/app/routers/verification.py, mounted at `/api/v1/verify` (backend/app/main.py).
//!
//! QR document verification.
//!
//! Public: the QR on every page of an issued document points at /v/<token>, which calls:
//!   GET /verify/t/{token}        state + frozen facts + approval record (login required for HR documents)
//!   GET /verify/t/{token}/pdf    the issued PDF, read-only (?download=true only when the policy allows)
//!
//! Internal:
//!   POST /verify/issue                         render + issue a verified PDF for an approved document
//!   GET  /verify/entity/{type}/{id}            codes issued for a document
//!   GET  /verify/{id}/pdf                      the rendered PDF
//!   POST /verify/{id}/revoke                   stop a code from verifying
//!   GET  /verify/{id}/views                    scan log
//!
//! Not behind INTERNAL_ONLY: the public routes take `OptionalUser`, the others `InternalUser`.
//!
//! Route order: Starlette tries `/t/{token}` before `/{verification_id}/...`, axum prefers the static `t`
//! segment; both pick `/t/{token}` for `GET /verify/t/x`. The one difference: `POST /verify/t/revoke`
//! (a verification id that is literally "t") is a 404 in Python (`/{verification_id}/revoke`, invalid id)
//! and a 405 here (axum matched the GET-only `/t/{token}` path).

use std::sync::atomic::{AtomicI64, Ordering};

use axum::extract::Path;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use redis::AsyncCommands;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, InternalUser, OptionalUser};
use granary_core::db::Db;
use granary_core::validation::{v, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::verification::DocumentVerification;
use granary_services::approvals::engine;
use granary_services::verification::{render, service};

use super::at;
use crate::extract::ClientIp;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/verify";

/// `PUBLIC_RATE_LIMIT`: public lookups per IP per minute (a static so tests can raise it, as the Python
/// tests monkeypatch the module constant).
pub static PUBLIC_RATE_LIMIT: AtomicI64 = AtomicI64::new(30);

/// `RESTRICTED_VIEW_PERMS`: beyond verification:view_log, who may open a login-gated code per document type.
fn restricted_view_perm(entity_type: &str) -> Option<&'static str> {
    match entity_type {
        "offer_letter" => Some("hr:manage_applicants"),
        "payroll_run" => Some("hr:manage_compensation"),
        _ => None,
    }
}

/// The routes of `router` in verification.py, in its order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/t/{token}"), get(verify_by_token))
        .route(&at(PREFIX, "/t/{token}/pdf"), get(verified_pdf))
        .route(&at(PREFIX, "/issue"), post(issue_verification))
        .route(
            &at(PREFIX, "/entity/{entity_type}/{entity_id}"),
            get(verifications_for_document),
        )
        .route(&at(PREFIX, "/{verification_id}/pdf"), get(internal_pdf))
        .route(
            &at(PREFIX, "/{verification_id}/revoke"),
            post(revoke_verification),
        )
        .route(
            &at(PREFIX, "/{verification_id}/views"),
            get(verification_views),
        )
}

/// `_require(user, *codes)`: superadmin, or any of `codes`; the 403 names the first.
fn _require(user: &AuthUser, codes: &[&str]) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !codes.iter().any(|c| user.perms.contains(*c)) {
        return Err(ApiError::http(
            403,
            format!("Permission required: {}", codes[0]),
        ));
    }
    Ok(())
}

/// `_rate_limit(request)`: per IP per minute; never locks people out when Redis is unavailable.
async fn _rate_limit(ip: &ClientIp) -> ApiResult<()> {
    let key = format!(
        "verify:rl:{}:{}",
        ip.0.as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown"),
        Utc::now().format("%Y%m%d%H%M")
    );
    let counted = async {
        let mut conn = granary_core::redis_client::get_redis().conn().await?;
        let count: i64 = conn.incr(&key, 1).await?;
        if count == 1 {
            let _: () = conn.expire(&key, 120).await?;
        }
        Ok::<i64, redis::RedisError>(count)
    }
    .await;
    let Ok(count) = counted else { return Ok(()) };
    if count > PUBLIC_RATE_LIMIT.load(Ordering::SeqCst) {
        return Err(ApiError::http(
            429,
            "Too many verification requests: try again in a minute",
        ));
    }
    Ok(())
}

/// The request facts `_log_view` reads.
struct Viewer {
    ip: Option<String>,
    user_agent: String,
}

fn viewer(ip: &ClientIp, headers: &HeaderMap) -> Viewer {
    // Starlette decodes header values as latin-1; `[:1000]` counts code points.
    let ua: String = headers
        .get("user-agent")
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect::<String>())
        .unwrap_or_default();
    Viewer {
        ip: ip.0.clone(),
        user_agent: ua.chars().take(1000).collect(),
    }
}

/// `_log_view(db, v, request, user, outcome)`.
async fn _log_view(
    db: &mut PgConnection,
    v: &DocumentVerification,
    who: &Viewer,
    user: Option<&AuthUser>,
    outcome: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO document_verification_views (verification_id, user_id, ip_address, user_agent, outcome, id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(v.id)
    .bind(user.map(|u| u.id))
    .bind(&who.ip)
    .bind(&who.user_agent)
    .bind(outcome)
    .bind(Uuid::new_v4())
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `uuid.UUID(text)`, or None.
fn py_uuid(text: &str) -> Option<Uuid> {
    crate::extract::parse_uuid(text, "ID").ok()
}

/// `_by_token(db, token)`.
async fn _by_token(db: &mut PgConnection, token: &str) -> ApiResult<DocumentVerification> {
    let row = match py_uuid(token) {
        Some(uid) => {
            let rows: Vec<DocumentVerification> =
                sqlx::query_as("SELECT * FROM document_verifications WHERE token = $1 LIMIT 2")
                    .bind(uid)
                    .fetch_all(&mut *db)
                    .await?;
            if rows.len() > 1 {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "Multiple rows were found when one or none was required"
                )));
            }
            rows.into_iter().next()
        }
        None => None,
    };
    row.ok_or_else(|| ApiError::http(404, "This verification code is not recognised"))
}

/// `_check_access(db, v, request, user)`: a refused attempt is logged and committed before the error.
async fn _check_access(
    db: &Db,
    v: &DocumentVerification,
    who: &Viewer,
    user: Option<&AuthUser>,
) -> ApiResult<()> {
    if !v.requires_login {
        return Ok(());
    }
    let Some(u) = user.filter(|u| u.portal_type == "internal") else {
        {
            let mut conn = db.conn().await?;
            _log_view(&mut conn, v, who, user, "login_required").await?;
        }
        db.commit().await?;
        return Err(ApiError::http_json(
            401,
            json!({"code": "login_required",
                   "message": "Sign in with your company account to view this document"}),
        ));
    };
    let restricted = restricted_view_perm(&v.entity_type);
    let allowed = u.is_superadmin
        || u.perms.contains("verification:view_log")
        || restricted.is_some_and(|p| u.perms.contains(p));
    if !allowed {
        {
            let mut conn = db.conn().await?;
            _log_view(&mut conn, v, who, user, "forbidden").await?;
        }
        db.commit().await?;
        return Err(ApiError::http(
            403,
            "Your account is not permitted to view this document",
        ));
    }
    Ok(())
}

/// `re.sub(r"[^A-Za-z0-9._-]+", "_", v.document_number or "document") + ".pdf"`.
fn pdf_filename(number: Option<&str>) -> String {
    let src = number.filter(|n| !n.is_empty()).unwrap_or("document");
    let mut out = String::new();
    let mut in_run = false;
    for c in src.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    out + ".pdf"
}

// ─── Public ───────────────────────────────────────────────────────────────────

/// `GET /t/{token}`.
pub async fn verify_by_token(
    Path(token): Path<String>,
    db: Db,
    OptionalUser(user): OptionalUser,
    ip: ClientIp,
    headers: HeaderMap,
) -> ApiResult<Response> {
    _rate_limit(&ip).await?;
    let who = viewer(&ip, &headers);
    let mut v = {
        let mut conn = db.conn().await?;
        _by_token(&mut conn, &token).await?
    };
    _check_access(&db, &v, &who, user.as_ref()).await?;
    let mut conn = db.conn().await?;
    let state = service::evaluate(&mut conn, &mut v).await?;
    let now = Utc::now();
    v.view_count += 1;
    v.last_viewed_at = Some(now);
    sqlx::query(
        "UPDATE document_verifications SET view_count = $2, last_viewed_at = $3, updated_at = now() WHERE id = $1",
    )
    .bind(v.id)
    .bind(v.view_count)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    let outcome = if state == "valid" { "ok" } else { &state };
    _log_view(&mut conn, &v, &who, user.as_ref(), outcome).await?;
    let payload = service::public_payload(&mut conn, &v, &state).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(payload))
}

granary_core::py_model! {
    /// `download: bool = False`.
    pub struct PdfQ {
        pub download: bool = false,
    }
}

/// `GET /t/{token}/pdf`.
pub async fn verified_pdf(
    Path(token): Path<String>,
    db: Db,
    OptionalUser(user): OptionalUser,
    PyQuery(q): PyQuery<PdfQ>,
    ip: ClientIp,
    headers: HeaderMap,
) -> ApiResult<Response> {
    _rate_limit(&ip).await?;
    let who = viewer(&ip, &headers);
    let mut v = {
        let mut conn = db.conn().await?;
        _by_token(&mut conn, &token).await?
    };
    _check_access(&db, &v, &who, user.as_ref()).await?;
    let state = {
        let mut conn = db.conn().await?;
        service::evaluate(&mut conn, &mut v).await?
    };
    if state != "valid" {
        {
            let mut conn = db.conn().await?;
            _log_view(&mut conn, &v, &who, user.as_ref(), &state).await?;
        }
        db.commit().await?;
        return Err(ApiError::http_json(
            410,
            json!({"code": state, "message": "This document is no longer valid"}),
        ));
    }
    let has_pdf = v.pdf_object_key.as_deref().is_some_and(|k| !k.is_empty());
    if !has_pdf || v.view_scope != "metadata_document" {
        return Err(ApiError::http(
            404,
            "The document itself cannot be viewed from this code",
        ));
    }
    if q.download && !v.allow_download {
        return Err(ApiError::http(
            403,
            "Downloading this document is not allowed",
        ));
    }
    // the sealed outbound copy, the same file email and WhatsApp carry
    let (data, _) = {
        let mut conn = db.conn().await?;
        service::outbound_pdf(&mut conn, &mut v).await?
    };
    {
        let mut conn = db.conn().await?;
        let outcome = if q.download { "download" } else { "pdf" };
        _log_view(&mut conn, &v, &who, user.as_ref(), outcome).await?;
    }
    db.commit().await?;
    let filename = pdf_filename(v.document_number.as_deref());
    let disposition = if q.download { "attachment" } else { "inline" };
    Ok(resp::with_headers(
        resp::bytes_response(200, data, "application/pdf"),
        &[
            (
                "content-disposition",
                format!("{disposition}; filename=\"{filename}\""),
            ),
            ("cache-control", "no-store".into()),
            ("x-robots-tag", "noindex, nofollow".into()),
        ],
    ))
}

// ─── Internal ─────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class IssueBody(BaseModel)`.
    pub struct IssueBody {
        pub entity_type: String,
        pub entity_id: String,
    }
}

/// `POST /issue` (201).
pub async fn issue_verification(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<IssueBody>,
) -> ApiResult<Response> {
    _require(&user, &["verification:issue"])?;
    let v = {
        let mut conn = db.conn().await?;
        service::issue(&mut conn, &body.entity_type, &body.entity_id, &user).await?
    };
    db.commit().await?;
    Ok(resp::json_status(
        201,
        service::internal_dict(&v, true, Some("valid")),
    ))
}

/// `GET /entity/{entity_type}/{entity_id}`.
pub async fn verifications_for_document(
    Path((entity_type, entity_id)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    _require(
        &user,
        &[
            "verification:issue",
            "verification:view_log",
            "verification:revoke",
        ],
    )?;
    let Some(uid) = py_uuid(&entity_id) else {
        return Err(ApiError::http(404, "Document not found"));
    };
    let mut conn = db.conn().await?;
    let policy = engine::get_policy(&mut conn, &entity_type).await?;
    let rows: Vec<DocumentVerification> = sqlx::query_as(
        "SELECT * FROM document_verifications WHERE entity_type = $1 AND entity_id = $2 ORDER BY issued_at DESC",
    )
    .bind(&entity_type)
    .bind(uid)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for mut v in rows {
        let live = if v.status == "active" {
            service::evaluate(&mut conn, &mut v).await?
        } else {
            v.status.clone()
        };
        out.push(service::internal_dict(&v, true, Some(&live)));
    }
    drop(conn);
    db.commit().await?; // evaluate() may have marked codes expired
    Ok(resp::json(json!({
        "supported": render::supports(&entity_type),
        "qr_enabled": policy.as_ref().is_some_and(|p| p.row.qr_enabled),
        "verifications": out,
    })))
}

/// `_by_id(db, verification_id)`.
async fn _by_id(db: &mut PgConnection, verification_id: &str) -> ApiResult<DocumentVerification> {
    let row = match py_uuid(verification_id) {
        Some(uid) => {
            sqlx::query_as("SELECT * FROM document_verifications WHERE id = $1")
                .bind(uid)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    row.ok_or_else(|| ApiError::http(404, "Verification not found"))
}

/// `GET /{verification_id}/pdf`.
pub async fn internal_pdf(
    Path(verification_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    _require(&user, &["verification:issue", "verification:view_log"])?;
    let mut v = {
        let mut conn = db.conn().await?;
        _by_id(&mut conn, &verification_id).await?
    };
    if v.pdf_object_key.as_deref().is_none_or(|k| k.is_empty()) {
        return Err(ApiError::http(
            404,
            "No rendered document for this verification",
        ));
    }
    // the copy that went out (sealed), so staff download what the customer holds
    let (data, _) = {
        let mut conn = db.conn().await?;
        service::outbound_pdf(&mut conn, &mut v).await?
    };
    let filename = pdf_filename(v.document_number.as_deref());
    Ok(resp::with_headers(
        resp::bytes_response(200, data, "application/pdf"),
        &[
            (
                "content-disposition",
                format!("attachment; filename=\"{filename}\""),
            ),
            ("cache-control", "no-store".into()),
        ],
    ))
}

granary_core::py_model! {
    /// `class RevokeBody(BaseModel)`.
    pub struct RevokeBody {
        pub reason: String => v::str_c(v::StrC::new().min_length(3).max_length(1000)),
    }
}

/// `POST /{verification_id}/revoke`.
pub async fn revoke_verification(
    Path(verification_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<RevokeBody>,
) -> ApiResult<Response> {
    _require(&user, &["verification:revoke"])?;
    let v = {
        let mut conn = db.conn().await?;
        let mut v = _by_id(&mut conn, &verification_id).await?;
        let reason = granary_compat::text::pystr::strip(&body.reason).to_string();
        service::revoke(&mut conn, &mut v, &user, &reason).await?;
        v
    };
    db.commit().await?;
    Ok(resp::json(service::internal_dict(
        &v,
        true,
        Some("revoked"),
    )))
}

granary_core::py_model! {
    /// `limit: int = 200`.
    pub struct ViewsQ {
        pub limit: i64 = 200,
    }
}

/// `GET /{verification_id}/views`.
pub async fn verification_views(
    Path(verification_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ViewsQ>,
) -> ApiResult<Response> {
    _require(&user, &["verification:view_log"])?;
    let mut conn = db.conn().await?;
    let v = _by_id(&mut conn, &verification_id).await?;
    let limit = q.limit.clamp(1, 1000);
    type Row = (
        Option<chrono::DateTime<Utc>>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT document_verification_views.created_at, document_verification_views.outcome, users.name, \
         document_verification_views.ip_address, document_verification_views.user_agent \
         FROM document_verification_views LEFT OUTER JOIN users ON users.id = document_verification_views.user_id \
         WHERE document_verification_views.verification_id = $1 \
         ORDER BY document_verification_views.created_at DESC LIMIT $2",
    )
    .bind(v.id)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(at, outcome, name, ip, ua)| {
            json!({"at": resp::iso_opt(at.as_ref()), "outcome": outcome, "user": name,
                   "ip_address": ip, "user_agent": ua})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}
