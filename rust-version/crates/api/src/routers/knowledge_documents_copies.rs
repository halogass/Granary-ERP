//! doclib A2: the copies of library documents. Every copy that leaves the server is stamped (footer on
//! every page, hidden fingerprint) and logged in library_downloads.
//!
//!   GET  /api/v1/knowledge/documents/{id}/view?version=          inline PDF/image, staff footer (staff_download, web)
//!   GET  /api/v1/knowledge/documents/{id}/download?version=&source=1   attachment, staff footer; source=1 the
//!                                                                 DOCX/XLSX/PPTX with the fingerprint in its properties
//!                                                                 (editors only, source_download)
//!   POST /api/v1/knowledge/documents/trace                       multipart `file` or JSON {fingerprint}: who got that
//!                                                                 copy (doc admins only); null when unknown
//!   POST /api/v1/knowledge/documents/price-list/generate         the internal price list as a new live version
//!                                                                 (documents:create)
//!   GET  /api/v1/partner/documents                               partner portal: partner documents, live only
//!   GET  /api/v1/partner/documents/{id}/download                 partner portal: stamped with the partner's company
//!
//! Version rules: without `version` the current live one. Another version only for doc admins (history), except
//! the version waiting for approval, which its editors and the approvers may open. Paths are absolute; merged
//! into the knowledge router by knowledge_documents::router (doclib A1).

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser, InternalUser};
use granary_core::db::Db;
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::doc_library::fingerprint::{self, Kind, TraceHit, Who};
use granary_services::doc_library::{
    self as lib, access, deliver, price_list, view, Document, Filters, Scope, Version,
};

use super::at;
use crate::extract::{ClientIp, FormValue, PyForm};
use crate::resp;

const KB: &str = "/api/v1/knowledge";
const PARTNER: &str = "/api/v1/partner";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(KB, "/documents/{id}/view"), get(view_document))
        .route(&at(KB, "/documents/{id}/download"), get(download_document))
        .route(&at(KB, "/documents/trace"), post(trace_copy))
        .route(
            &at(KB, "/documents/price-list/generate"),
            post(generate_price_list),
        )
        .route(&at(PARTNER, "/documents"), get(partner_documents))
        .route(
            &at(PARTNER, "/documents/{id}/download"),
            get(partner_download),
        )
}

fn not_found() -> ApiError {
    ApiError::http(404, "Document not found")
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect::<String>())
        .map(|s| s.chars().take(500).collect())
}

fn truthy(v: Option<&String>) -> bool {
    matches!(
        v.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn require_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains(access::PERM_VIEW) || access::is_doc_admin(user) {
        Ok(())
    } else {
        Err(ApiError::http(403, "Permission required: documents:view"))
    }
}

/// The version a staff member asked for (see the module doc for the rules).
async fn pick_version(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: &Document,
    wanted: Option<&str>,
) -> ApiResult<Version> {
    let wanted = wanted.map(str::trim).filter(|s| !s.is_empty());
    let Some(raw) = wanted else {
        return match lib::current_live(db, doc.id).await? {
            Some(v) => Ok(v),
            None => {
                // nothing live yet: editors and approvers may open the pending one
                match lib::pending_version(db, doc.id).await? {
                    Some(p) if may_open_pending(db, user, doc).await? => Ok(p),
                    _ => Err(ApiError::http(404, "This document has no live version yet")),
                }
            }
        };
    };
    let vid = Uuid::parse_str(raw).map_err(|_| ApiError::http(404, "Version not found"))?;
    let v = lib::load_version(db, vid)
        .await?
        .filter(|v| v.document_id == doc.id)
        .ok_or_else(|| ApiError::http(404, "Version not found"))?;
    if doc.current_version_id == Some(v.id) && v.status == lib::STATUS_LIVE {
        return Ok(v);
    }
    if access::is_doc_admin(user) {
        return Ok(v);
    }
    if v.status == lib::STATUS_PENDING && may_open_pending(db, user, doc).await? {
        return Ok(v);
    }
    Err(ApiError::http(
        403,
        "Only directors and document admins can open older versions",
    ))
}

async fn may_open_pending(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: &Document,
) -> ApiResult<bool> {
    Ok(access::can_approve(user) || access::can_edit(db, user, doc).await?)
}

/// Read, stamp with the staff footer, log; the response with the given disposition.
async fn staff_copy(
    db: &mut PgConnection,
    user: &AuthUser,
    v: &Version,
    source: bool,
    disposition: &str,
    ip: Option<String>,
    ua: Option<String>,
) -> ApiResult<Response> {
    let (bytes, name, mime, kind) = if source {
        match (&v.source_object_key, &v.source_file_name, &v.source_mime) {
            (Some(k), Some(n), Some(m)) => (
                lib::read_object(k).await?,
                n.clone(),
                m.clone(),
                Kind::SourceDownload,
            ),
            _ => {
                return Err(ApiError::http(
                    404,
                    "This version has no editable source file",
                ))
            }
        }
    } else {
        // 5 Nov 2026: the version's own copy, or the CRM attachment it references (store once)
        (
            lib::read_file(v).await?,
            v.file_name.clone(),
            v.file_mime.clone(),
            Kind::StaffDownload,
        )
    };
    let who = Who {
        user_id: Some(user.id),
        name: Some(user.name.clone()),
        channel: Some("web".into()),
        ip,
        user_agent: ua,
        ..Default::default()
    };
    let (data, issued) = fingerprint::stamped_copy(db, kind, v, &who, &bytes, &mime).await?;
    // security 26 Oct: a stored file's type is kept only when it cannot run as a page
    let r = crate::files::safe_file_response(data, &mime, &name, disposition == "inline");
    Ok(resp::with_headers(
        r,
        &[
            ("cache-control", "private, no-store".into()),
            ("x-document-fingerprint", issued.id),
            (
                "access-control-expose-headers",
                "content-disposition, x-document-fingerprint".into(),
            ),
        ],
    ))
}

/// `GET /documents/{id}/view?version=`: inline, for the browser viewer.
pub async fn view_document(
    Path(id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    ip: ClientIp,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let id = Uuid::parse_str(id.trim()).map_err(|_| not_found())?;
    let mut conn = db.conn().await?;
    let doc = access::viewable(&mut conn, &user, id).await?;
    let v = pick_version(&mut conn, &user, &doc, q.get("version").map(String::as_str)).await?;
    staff_copy(
        &mut conn,
        &user,
        &v,
        false,
        "inline",
        ip.0,
        user_agent(&headers),
    )
    .await
}

/// `GET /documents/{id}/download?version=&source=1`: an attachment.
pub async fn download_document(
    Path(id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    ip: ClientIp,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let id = Uuid::parse_str(id.trim()).map_err(|_| not_found())?;
    let mut conn = db.conn().await?;
    let doc = access::viewable(&mut conn, &user, id).await?;
    let source = truthy(q.get("source"));
    if source && !access::can_edit(&mut conn, &user, &doc).await? {
        return Err(ApiError::http(
            403,
            "Only the document's owner or a director can download the editable source",
        ));
    }
    let v = pick_version(&mut conn, &user, &doc, q.get("version").map(String::as_str)).await?;
    staff_copy(
        &mut conn,
        &user,
        &v,
        source,
        "attachment",
        ip.0,
        user_agent(&headers),
    )
    .await
}

/// The trace result for the screen: the nested hit plus the flat fields the frontend reads.
fn hit_json(h: &TraceHit) -> Value {
    let mut v = serde_json::to_value(h).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("document_id".into(), json!(h.document.id));
        o.insert("document_title".into(), json!(h.document.name));
        o.insert("version_id".into(), json!(h.version.id));
        o.insert("version_label".into(), json!(h.version.label));
        o.insert(
            "user_name".into(),
            json!(h.user.as_ref().map(|u| u.name.clone())),
        );
        o.insert(
            "partner_name".into(),
            json!(h.partner_account.as_ref().map(|p| p.name.clone())),
        );
    }
    v
}

/// `POST /documents/trace`: multipart `file`, or JSON `{fingerprint}`. Doc admins only.
pub async fn trace_copy(
    InternalUser(user): InternalUser,
    State(state): State<AppState>,
    db: Db,
    req: Request,
) -> ApiResult<Response> {
    access::require_doc_admin(&user)?;
    let ctype = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut conn = db.conn().await?;
    let hit = if ctype.starts_with("multipart/form-data") {
        let form = PyForm::from_request(req, &state).await?;
        let file = form.items.iter().rev().find_map(|(k, v)| match v {
            FormValue::File(f) if k == "file" => Some(f.clone()),
            _ => None,
        });
        match (file, form.opt_text("fingerprint")) {
            (Some(f), _) if !f.data.is_empty() => {
                fingerprint::trace_file(&mut conn, &f.data).await?
            }
            (_, Some(fp)) if !fp.trim().is_empty() => fingerprint::trace_id(&mut conn, &fp).await?,
            _ => {
                return Err(ApiError::http(
                    422,
                    "Send the leaked file (file) or its id (fingerprint)",
                ))
            }
        }
    } else {
        let body = Bytes::from_request(req, &state).await.map_err(|_| {
            ApiError::http(422, "Send the leaked file (file) or its id (fingerprint)")
        })?;
        let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let fp = v
            .get("fingerprint")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ApiError::http(422, "Send the leaked file (file) or its id (fingerprint)")
            })?;
        fingerprint::trace_id(&mut conn, fp).await?
    };
    Ok(resp::json(match hit {
        Some(h) => hit_json(&h),
        None => Value::Null,
    }))
}

/// `POST /documents/price-list/generate`: the price list as a new live internal version.
pub async fn generate_price_list(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let (doc, _v) = price_list::generate(&mut conn, &user).await?;
    Ok(resp::json(view::detail(&mut conn, &user, &doc).await?))
}

// ─── partner portal ───────────────────────────────────────────────────────────

/// The calling partner portal user's account: (id, company name). 403 for anyone else.
async fn portal_account(db: &mut PgConnection, user: &AuthUser) -> ApiResult<(Uuid, String)> {
    let pid = match (user.portal_type.as_str(), user.partner_account_id) {
        ("partner", Some(p)) => p,
        _ => return Err(ApiError::http(403, "Partner portal access required")),
    };
    let row: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, company_name FROM partner_accounts WHERE id = $1 AND is_deleted = false",
    )
    .bind(pid)
    .fetch_optional(&mut *db)
    .await?;
    let (id, name) =
        row.ok_or_else(|| ApiError::http(403, "Partner account not found or removed"))?;
    Ok((
        id,
        name.filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Partner".into()),
    ))
}

/// What a partner sees of a document: no owner, no pending version, no internal variants.
fn partner_item(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        for k in ["owner", "pending_version", "language_group_id"] {
            o.insert(k.into(), Value::Null);
        }
        o.insert("language_variants".into(), json!([]));
        o.insert("can_edit".into(), json!(false));
    }
    v
}

/// `GET /api/v1/partner/documents`: partner documents with a live version (category roles do not apply).
pub async fn partner_documents(
    CurrentUser(user): CurrentUser,
    db: Db,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    portal_account(&mut conn, &user).await?;
    let s = |k: &str| {
        q.get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let filters = Filters {
        category: s("category"),
        language: s("language"),
        limit: s("limit").and_then(|v| v.parse().ok()).or(Some(200)),
        offset: s("offset").and_then(|v| v.parse().ok()),
        only_live: true,
        ..Default::default()
    };
    let res = lib::search(
        &mut conn,
        &Scope::Audiences(vec!["partner".into()]),
        s("q").as_deref(),
        &filters,
    )
    .await?;
    let mut items = Vec::with_capacity(res.items.len());
    for d in &res.items {
        items.push(partner_item(view::item(&mut conn, None, d).await?));
    }
    Ok(resp::json(json!({"items": items, "total": res.total})))
}

/// `GET /api/v1/partner/documents/{id}/download`: the live version stamped for the partner's company.
pub async fn partner_download(
    Path(id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    ip: ClientIp,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let (pid, company) = portal_account(&mut conn, &user).await?;
    let id = Uuid::parse_str(id.trim()).map_err(|_| not_found())?;
    let doc = lib::load_document(&mut conn, id)
        .await?
        .filter(|d| d.has_audience("partner") && d.kind != "price_list")
        .ok_or_else(not_found)?;
    let v = lib::current_live(&mut conn, doc.id)
        .await?
        .ok_or_else(not_found)?;
    let out = deliver::partner_copy(
        &mut conn,
        &v,
        pid,
        &company,
        user.id,
        ip.0,
        user_agent(&headers),
    )
    .await?;
    let r =
        crate::files::safe_file_response(out.file.data, &out.file.mime, &out.file.filename, false);
    Ok(resp::with_headers(
        r,
        &[("cache-control", "private, no-store".into())],
    ))
}
