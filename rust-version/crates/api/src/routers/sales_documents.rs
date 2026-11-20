//! backend/app/routers/sales_documents.py, mounted at `/api/v1/sales` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Sales documents in the ERP (Nary Phase 4): Solution Proposals. Approval, verification QR and sending use
//! the shared endpoints (/approvals, /verify, /documents/{type}/{id}/send) with entity type
//! "sales_document". Ids in the path go through `uuid.UUID(...)` in Python: a malformed one is a
//! ValueError on a router line (the central 422 "Invalid id format"), not a 400.

use crate::extract::router_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{Cx, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::documents::{NaryRateCard, SalesDocument, SalesDocumentAsset};
use granary_services::documents::core_ as core;

use super::at;
use super::sales::{commit, WithSet};
use crate::extract::PyForm;
use crate::resp::{self, dec_str, iso, iso_opt};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/sales";

/// The routes of `router` in sales_documents.py. Paths are absolute (`super::at(PREFIX, "/...")`): several
/// Python routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/documents"),
            get(list_documents).post(create_document),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}"),
            get(get_document).patch(patch_document),
        )
        .route(&at(PREFIX, "/documents/{doc_id}/preview"), get(preview))
        .route(&at(PREFIX, "/documents/{doc_id}/blocks"), put(save_blocks))
        .route(
            &at(PREFIX, "/documents/{doc_id}/quotations"),
            post(link_quotation),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/assets"),
            post(upload_asset),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/assets/{asset_id}"),
            get(get_asset),
        )
        .route(&at(PREFIX, "/documents/{doc_id}/ask"), post(ask_nary))
        // fix-longapply: long jobs the editor polls; reject / reopen a proposal
        .route(
            &at(PREFIX, "/documents/{doc_id}/jobs/{job_id}"),
            get(get_job),
        )
        .route(&at(PREFIX, "/documents/{doc_id}/reject"), post(reject))
        .route(&at(PREFIX, "/documents/{doc_id}/reopen"), post(reopen))
        .route(&at(PREFIX, "/documents/{doc_id}/confirm"), post(confirm))
        .route(&at(PREFIX, "/documents/{doc_id}/revise"), post(revise))
        .route(
            &at(PREFIX, "/documents/{doc_id}/cost-sheet"),
            get(cost_sheet).post(reestimate),
        )
        .route(&at(PREFIX, "/rate-card"), get(rate_card).post(add_rate))
        .route(&at(PREFIX, "/rate-card/research"), post(research_rates))
        // visuals V2: charts and diagrams
        .route(
            &at(PREFIX, "/documents/{doc_id}/visuals/preview"),
            post(visual_preview),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/visuals/check"),
            post(visual_check),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/visuals/{block_id}/redraw"),
            post(visual_redraw),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/visuals/{block_id}/ask"),
            post(visual_ask),
        )
}

/// `_require(user, *codes)`: superadmin, or any of the codes; the 403 names the first.
fn require(user: &AuthUser, codes: &[&str]) -> ApiResult<()> {
    if user.is_superadmin || codes.iter().any(|c| user.perms.contains(*c)) {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        format!("Permission required: {}", codes[0]),
    ))
}

/// `_doc(db, doc_id)`: a live document (404 "Document not found").
async fn doc(db: &mut PgConnection, doc_id: &str) -> ApiResult<SalesDocument> {
    let id = router_uuid(doc_id)?;
    let d: Option<SalesDocument> = sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    match d {
        Some(d) if !d.is_deleted => Ok(d),
        _ => Err(ApiError::http(404, "Document not found")),
    }
}

/// The document row as a commit reloads it.
async fn reload(db: &mut PgConnection, id: Uuid) -> ApiResult<SalesDocument> {
    Ok(
        sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *db)
            .await?,
    )
}

/// `_out(db, doc, full=...)`.
async fn out(db: &mut PgConnection, d: &SalesDocument, full: bool) -> ApiResult<Value> {
    let mut o = json!({
        "id": d.id.to_string(), "doc_type": d.doc_type, "number": d.number, "full_number": d.full_number,
        "revision": d.revision, "revision_reason": d.revision_reason, "title": d.title, "subtitle": d.subtitle,
        "language": d.language, "status": d.status, "deal_id": d.deal_id.map(|u| u.to_string()),
        "organization_id": d.organization_id.map(|u| u.to_string()),
        "contact_id": d.contact_id.map(|u| u.to_string()), "validity_days": d.validity_days,
        "classification": d.classification, "cover": d.cover, "origin": d.origin,
        "created_at": iso(&d.created_at), "sent_at": iso_opt(d.sent_at.as_ref()),
    });
    if full {
        let blocks: Vec<Value> = core::blocks_of(db, d.id)
            .await?
            .iter()
            .map(|b| {
                json!({"id": b.id.to_string(), "section": b.section, "position": b.position,
                       "block_type": b.block_type, "data": b.data})
            })
            .collect();
        let quotations: Vec<Value> = core::quotations_of(db, d.id)
            .await?
            .iter()
            .map(|(q, label)| {
                json!({"id": q.id.to_string(),
                       "number": q.full_number.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| q.number.clone()),
                       "status": q.status, "currency": q.currency,
                       "total": q.total.as_ref().map(dec_str), "label": label})
            })
            .collect();
        let assets: Vec<SalesDocumentAsset> = sqlx::query_as(
            "SELECT * FROM sales_document_assets WHERE sales_document_assets.document_id = $1 \
             ORDER BY sales_document_assets.created_at",
        )
        .bind(d.id)
        .fetch_all(&mut *db)
        .await?;
        let assets: Vec<Value> = assets
            .iter()
            .map(|a| {
                json!({"id": a.id.to_string(), "kind": a.kind, "caption": a.caption, "status": a.status,
                       "checks": a.checks, "error": a.error, "filename": a.filename})
            })
            .collect();
        let template: Vec<Value> = core::template(db)
            .await?
            .iter()
            .map(|s| {
                json!({"key": s["key"], "title": s["title"],
                       "blocks": s.get("blocks").cloned().unwrap_or(Value::Null)})
            })
            .collect();
        o["blocks"] = Value::Array(blocks);
        o["quotations"] = Value::Array(quotations);
        o["assets"] = Value::Array(assets);
        o["template"] = Value::Array(template);
    }
    Ok(o)
}

granary_core::py_model! {
    /// The query parameters of `GET /documents`.
    pub struct DocumentListQuery {
        pub deal_id: Option<String> = None,
        pub include_superseded: bool = false,
        /// 10 Nov 2026: paging (it was a fixed newest 200, so older documents could not be listed)
        pub limit: i64 = 200,
        pub offset: i64 = 0,
    }
}

/// `GET /documents`: newest first, `limit` (default 200, at most 500) from `offset` (deal filter).
pub async fn list_documents(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(p): PyQuery<DocumentListQuery>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_quotations", "sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let mut sql =
        String::from("SELECT * FROM sales_documents WHERE sales_documents.is_deleted = false");
    let mut deal: Option<Uuid> = None;
    if let Some(d) = p.deal_id.as_deref().filter(|s| !s.is_empty()) {
        deal = Some(router_uuid(d)?);
        sql.push_str(" AND sales_documents.deal_id = $1");
    }
    if !p.include_superseded {
        sql.push_str(" AND sales_documents.status != 'superseded'");
    }
    let n = if deal.is_some() { 1 } else { 0 };
    sql.push_str(&format!(
        " ORDER BY sales_documents.created_at DESC, sales_documents.id LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut q = sqlx::query_as::<_, SalesDocument>(&sql);
    if let Some(d) = deal {
        q = q.bind(d);
    }
    q = q.bind(p.limit.clamp(1, 500)).bind(p.offset.max(0));
    let rows = q.fetch_all(&mut *conn).await?;
    let mut res = Vec::with_capacity(rows.len());
    for d in &rows {
        res.push(out(&mut conn, d, false).await?);
    }
    Ok(resp::json(Value::Array(res)))
}

granary_core::py_model! {
    /// `class DocumentIn(BaseModel)`.
    pub struct DocumentIn {
        pub title: String,
        pub subtitle: Option<String> = None,
        pub language: String = "en",
        pub deal_id: Option<String> = None,
        pub cover: Option<Map<String, Value>> = None,
    }
}

/// `POST /documents` (201): created from the template (manual).
pub async fn create_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<DocumentIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let deal: Option<granary_db::rows::crm::Deal> =
        match body.deal_id.as_deref().filter(|s| !s.is_empty()) {
            Some(d) => {
                let id = router_uuid(d)?;
                sqlx::query_as("SELECT * FROM deals WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            None => None,
        };
    let args = core::CreateArgs {
        user_id: Some(user.id),
        title: body.title.clone(),
        language: body.language.clone(),
        subtitle: body.subtitle.clone(),
        deal: deal.as_ref(),
        cover: body.cover.clone().map(Value::Object),
        ..Default::default()
    };
    let d = core::create(&mut conn, args).await?;
    commit(&mut conn).await?;
    let d = reload(&mut conn, d.id).await?;
    Ok(resp::json_status(201, out(&mut conn, &d, true).await?))
}

/// `GET /documents/{doc_id}`: document, blocks, quotations, assets, template.
pub async fn get_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_quotations", "sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    Ok(resp::json(out(&mut conn, &d, true).await?))
}

/// `GET /documents/{doc_id}/preview`: a draft PDF of the document as it stands (no QR: the verified copy
/// comes with approval).
pub async fn preview(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_quotations", "sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    // 23 Oct: the draft is built by documents::render::draft_job (shared with Nary's render_document_page)
    let entity = granary_services::approvals::adapters::Entity::SalesDocument(Box::new(d));
    let job = granary_services::documents::render::draft_job(&mut conn, "sales_document", &entity)
        .await?;
    drop(conn);
    let pdf = job.render().await?;
    let code = job.code;
    Ok(resp::with_headers(
        resp::bytes_response(200, pdf, "application/pdf"),
        &[(
            "content-disposition",
            format!("inline; filename=\"{}.pdf\"", code.replace('/', "-")),
        )],
    ))
}

granary_core::py_model! {
    /// `class BlocksIn(BaseModel)`.
    pub struct BlocksIn {
        pub blocks: Vec<Map<String, Value>>,
    }
}

/// `PUT /documents/{doc_id}/blocks`: save the editor (all blocks, in order).
pub async fn save_blocks(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<BlocksIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let blocks: Vec<Value> = body.blocks.into_iter().map(Value::Object).collect();
    core::replace_blocks(&mut conn, &d, &blocks).await?;
    commit(&mut conn).await?;
    let d = reload(&mut conn, d.id).await?;
    Ok(resp::json(out(&mut conn, &d, true).await?))
}

granary_core::py_model! {
    /// `class DocumentPatch(BaseModel)`.
    pub struct DocumentPatch {
        pub title: Option<String> = None,
        pub subtitle: Option<String> = None,
        pub language: Option<String> = None,
        pub cover: Option<Map<String, Value>> = None,
        pub validity_days: Option<i64> = None,
    }
}

/// `PATCH /documents/{doc_id}`: title, subtitle, language, cover, validity.
pub async fn patch_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<WithSet<DocumentPatch>>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let mut d = doc(&mut conn, &doc_id).await?;
    if !core::EDITABLE.contains(&d.status.as_str()) {
        return Err(ApiError::http(
            409,
            format!("A {} proposal can't be edited: revise it", d.status),
        ));
    }
    let m = &body.model;
    // setattr(doc, k, v) in field order; NULL in a NOT NULL column fails at commit (IntegrityError 409)
    let mut title: Option<String> = Some(d.title.clone());
    let mut language: Option<String> = Some(d.language.clone());
    let mut cover: Option<Value> = Some(d.cover.clone());
    let mut validity: Option<i64> = Some(d.validity_days as i64);
    for k in ["title", "subtitle", "language", "cover", "validity_days"] {
        if !body.is_set(k) {
            continue;
        }
        match k {
            "title" => title = m.title.clone(),
            "subtitle" => d.subtitle = m.subtitle.clone(),
            "language" => {
                if m.language.as_deref() != Some("en") {
                    return Err(ApiError::http(422, "language is en"));
                }
                language = m.language.clone();
            }
            // JSONB: None is the JSON null (NOT NULL holds, as in Python)
            "cover" => cover = Some(m.cover.clone().map(Value::Object).unwrap_or(Value::Null)),
            _ => {
                let v = m.validity_days.unwrap_or(0);
                if !(0 < v && v <= 365) {
                    return Err(ApiError::http(422, "validity_days is 1 to 365"));
                }
                validity = m.validity_days;
            }
        }
    }
    sqlx::query(
        "UPDATE sales_documents SET title = $2, subtitle = $3, language = $4, cover = $5, validity_days = $6::int8, \
         updated_at = CASE WHEN (title, subtitle, language, cover, validity_days) IS DISTINCT FROM \
         ($2::varchar, $3::varchar, $4::varchar, $5::jsonb, $6::int8::int4) THEN now() ELSE updated_at END \
         WHERE id = $1",
    )
    .bind(d.id)
    .bind(&title)
    .bind(&d.subtitle)
    .bind(&language)
    .bind(&cover)
    .bind(validity)
    .execute(&mut *conn)
    .await?;
    commit(&mut conn).await?;
    let d = reload(&mut conn, d.id).await?;
    Ok(resp::json(out(&mut conn, &d, true).await?))
}

granary_core::py_model! {
    /// `class LinkIn(BaseModel)`.
    pub struct LinkIn {
        pub quotation_id: String,
        pub label: Option<String> = None,
    }
}

/// `POST /documents/{doc_id}/quotations`: link a quotation (an option, a split).
pub async fn link_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<LinkIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    core::link_quotation(&mut conn, &d, &body.quotation_id, body.label.as_deref()).await?;
    commit(&mut conn).await?;
    let d = reload(&mut conn, d.id).await?;
    Ok(resp::json(out(&mut conn, &d, true).await?))
}

/// `s[:n]` (code points).
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `POST /documents/{doc_id}/assets` (201): upload an image.
pub async fn upload_asset(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.expect("validated");
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let key = granary_services::documents::assets::put(
        d.id,
        &file.data,
        file.content_type.as_deref().unwrap_or(""),
    )
    .await?;
    let id = Uuid::new_v4();
    let caption = head(&file.filename, 500);
    sqlx::query(
        "INSERT INTO sales_document_assets (id, document_id, kind, filename, mime, storage_key, caption, status, \
         checks, attempts, promoted, created_by_id) VALUES ($1, $2, 'upload', $3, $4, $5, $6, 'ready', '[]'::jsonb, \
         0, false, $7)",
    )
    .bind(id)
    .bind(d.id)
    .bind(head(&file.filename, 255))
    .bind(&file.content_type)
    .bind(&key)
    .bind(&caption)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    commit(&mut conn).await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "kind": "upload", "caption": caption, "status": "ready"}),
    ))
}

/// `GET /documents/{doc_id}/assets/{asset_id}`: the image (for the editor).
pub async fn get_asset(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((doc_id, asset_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_quotations", "sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let id = router_uuid(&asset_id)?;
    let row: Option<SalesDocumentAsset> =
        sqlx::query_as("SELECT * FROM sales_document_assets WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    drop(conn);
    // `str(row.document_id) != doc_id`: an exact string comparison
    let row = match row {
        Some(r)
            if r.document_id.map(|u| u.to_string()).as_deref() == Some(doc_id.as_str())
                && r.storage_key.as_deref().is_some_and(|k| !k.is_empty()) =>
        {
            r
        }
        _ => return Err(ApiError::http(404, "Image not found")),
    };
    let data =
        granary_services::documents::assets::get(row.storage_key.as_deref().unwrap_or("")).await?;
    let mime = row
        .mime
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "image/png".into());
    // visuals V2: a figure's SVG is the server's own drawing; still, nothing in it may run
    let mut headers = vec![("cache-control", "private, max-age=3600".to_string())];
    if mime == "image/svg+xml" {
        headers.push((
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'; sandbox".to_string(),
        ));
    }
    Ok(resp::with_headers(
        resp::bytes_response(200, data, &mime),
        &headers,
    ))
}

granary_core::py_model! {
    /// `class AskIn(BaseModel)`.
    pub struct AskIn {
        pub text: String,
    }
}

/// `POST /documents/{doc_id}/ask`: ask Nary to change something (Opus rewrites the affected blocks).
pub async fn ask_nary(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<AskIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let d = {
        let mut conn = db.conn().await?;
        doc(&mut conn, &doc_id).await?
    };
    let (text, u) = (body.text, user.clone());
    run_job(
        &db,
        &user,
        &d,
        "ask",
        job(move |conn, d| {
            Box::pin(async move {
                let res = granary_services::nary::proposal_edits::apply(conn, &d, &text, Some(&u))
                    .await?;
                commit(conn).await?;
                let d = reload(conn, d.id).await?;
                let mut o = match res {
                    Value::Object(m) => m,
                    _ => Map::new(),
                };
                o.insert("document".into(), out(conn, &d, true).await?);
                Ok((200, Value::Object(o)))
            })
        }),
    )
    .await
}

granary_core::py_model! {
    /// `class ConfirmIn(BaseModel)`.
    pub struct ConfirmIn {
        pub lines: Vec<Map<String, Value>>,
    }
}

/// `POST /documents/{doc_id}/confirm`: someone who may create quotations (sales:create_quotation) confirms the
/// proposal; Nary drafts the matching quotation (which then goes through the quotation approval).
pub async fn confirm(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<ConfirmIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let lines: Vec<Value> = body.lines.into_iter().map(Value::Object).collect();
    let res =
        granary_services::nary::proposals::confirm(&mut conn, &d, Some(&user), &lines).await?;
    Ok(resp::json(res))
}

granary_core::py_model! {
    /// `class ReviseIn(BaseModel)`.
    pub struct ReviseIn {
        pub reason: String,
    }
}

/// `POST /documents/{doc_id}/revise` (201): a new revision (supersedes this one and its quotations).
pub async fn revise(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<ReviseIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let mut d = doc(&mut conn, &doc_id).await?;
    let (new, changes) = core::revise_with_changes(&mut conn, &mut d, &user, &body.reason).await?;
    commit(&mut conn).await?;
    let new = reload(&mut conn, new.id).await?;
    let mut body = out(&mut conn, &new, true).await?;
    // 1 Nov 2026: the quotation copies are dated today and repriced; what changed per line
    if let Value::Object(m) = &mut body {
        m.insert("quotation_changes".into(), Value::Array(changes));
    }
    Ok(resp::json_status(201, body))
}

/// `GET /documents/{doc_id}/cost-sheet`: the internal cost sheet.
pub async fn cost_sheet(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_costs"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let sheet = granary_services::nary::costing::latest(&mut conn, d.id).await?;
    Ok(resp::json(json!({
        "sheet": sheet.as_ref().map(|s| s.content.clone()),
        "status": sheet.as_ref().map(|s| s.status.clone()),
        "created_at": sheet.as_ref().map(|s| iso(&s.created_at)),
    })))
}

/// `POST /documents/{doc_id}/cost-sheet`: re-estimate.
pub async fn reestimate(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_costs"])?;
    let d = {
        let mut conn = db.conn().await?;
        doc(&mut conn, &doc_id).await?
    };
    run_job(
        &db,
        &user,
        &d,
        "reestimate",
        job(move |conn, d| {
            Box::pin(async move {
                let blocks = core::blocks_of(conn, d.id).await?;
                let scope_v = json!({
                    "title": d.title,
                    "blocks": blocks.iter().map(|b| json!([b.section, b.block_type, b.data])).collect::<Vec<_>>(),
                });
                let scope = dumps(&PyValue::from(&scope_v), &DumpOpts::unicode())
                    .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
                let sheet = granary_services::nary::costing::estimate(conn, &d, &scope, None).await?;
                commit(conn).await?;
                Ok((200, json!({"sheet": sheet.content, "status": sheet.status})))
            })
        }),
    )
    .await
}

/// `GET /rate-card`.
pub async fn rate_card(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require(&user, &["sales:view_costs"])?;
    let mut conn = db.conn().await?;
    let rows: Vec<NaryRateCard> = sqlx::query_as(
        "SELECT * FROM nary_rate_card ORDER BY nary_rate_card.role, nary_rate_card.seniority, nary_rate_card.valid_from",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({"id": r.id.to_string(), "role": r.role, "seniority": r.seniority, "cost_base": dec_str(&r.cost_base),
                   "source": r.source, "valid_from": resp::date_iso(&r.valid_from), "status": r.status,
                   "proposed_by_nary": r.proposed_by_nary})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class RateIn(BaseModel)`.
    pub struct RateIn {
        pub role: String,
        pub seniority: String,
        pub cost_base: String,
        pub source: String,
        pub valid_from: NaiveDate,
    }
}

/// `POST /rate-card` (201): a new rate (it applies from valid_from; the previous active one for the role
/// and seniority is retired).
pub async fn add_rate(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<RateIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:manage_costs"])?;
    if !matches!(body.seniority.as_str(), "junior" | "mid" | "senior") {
        return Err(ApiError::http(422, "seniority is junior, mid or senior"));
    }
    // Decimal(str): nan/inf parse (and are not > 0 / fail the column)
    let cost: Decimal = match granary_core::validation::types::PyDec::parse(&body.cost_base) {
        None => return Err(ApiError::http(422, "cost_base must be a number")),
        Some(p) => match p.to_decimal() {
            Some(d) => d,
            // NaN / Infinity: `cost <= 0` is False for NaN (an InvalidOperation for sNaN); Postgres then
            // refuses the value. Both end as a 500.
            None => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "non-finite cost_base {}",
                    body.cost_base
                )))
            }
        },
    };
    if cost <= Decimal::ZERO {
        return Err(ApiError::http(422, "cost_base must be positive"));
    }
    let mut conn = db.conn().await?;
    sqlx::query(
        "UPDATE nary_rate_card SET status = 'retired', updated_at = now() WHERE role = $1 AND seniority = $2 \
         AND status = 'active'",
    )
    .bind(&body.role)
    .bind(&body.seniority)
    .execute(&mut *conn)
    .await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_rate_card (id, role, seniority, cost_base, source, valid_from, status, proposed_by_nary, \
         approved_by_id) VALUES ($1, $2, $3, $4, $5, $6, 'active', false, $7)",
    )
    .bind(id)
    .bind(head(pystr::strip(&body.role), 80))
    .bind(&body.seniority)
    .bind(cost)
    .bind(pystr::strip(&body.source))
    .bind(body.valid_from)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    commit(&mut conn).await?;
    Ok(resp::json_status(201, json!({"id": id.to_string()})))
}

/// `POST /rate-card/research`: Nary researches current salary references and proposes rate changes as
/// cards (the Owner approves each).
pub async fn research_rates(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require(&user, &["sales:manage_costs"])?;
    let mut conn = db.conn().await?;
    let res = granary_services::nary::rates::research(&mut conn, &user).await?;
    Ok(resp::json(res))
}

// ------------------------------------------------------------------------------------- visuals V2

granary_core::py_model! {
    /// The body of `POST /documents/{doc_id}/visuals/preview`.
    pub struct VisualPreviewIn {
        pub spec: Map<String, Value>,
        pub block_type: Option<String> = None,
        pub size: Option<String> = None,
    }
}

fn figure_size(s: Option<&str>) -> granary_services::visuals::FigureSize {
    if s == Some("half") {
        granary_services::visuals::FigureSize::Half
    } else {
        granary_services::visuals::FigureSize::Full
    }
}

fn figure_type(bt: Option<&str>) -> ApiResult<Option<&str>> {
    match bt {
        None | Some("") => Ok(None),
        Some(t) if granary_services::documents::figures::is_figure(t) => Ok(Some(t)),
        Some(t) => Err(ApiError::http(
            422,
            format!("'{t}' is not a figure block type (chart or diagram)"),
        )),
    }
}

/// `POST /documents/{doc_id}/visuals/preview`: guard and draw a spec, nothing saved.
/// {svg, png_base64, warnings, spec, block_type, kind}; a refusal is a 422 naming the problems.
pub async fn visual_preview(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<VisualPreviewIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_quotations", "sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let spec = Value::Object(body.spec);
    let out = granary_services::documents::figures::preview(
        &mut conn,
        Some(&d),
        figure_type(body.block_type.as_deref())?,
        &spec,
        figure_size(body.size.as_deref()),
    )
    .await?;
    Ok(resp::json(out))
}

granary_core::py_model! {
    /// The body of `POST /documents/{doc_id}/visuals/check`.
    pub struct VisualCheckIn {
        pub spec: Map<String, Value>,
        pub brief: Option<String> = None,
        pub caption: Option<String> = None,
        pub block_id: Option<String> = None,
        pub block_type: Option<String> = None,
        pub size: Option<String> = None,
    }
}

/// `POST /documents/{doc_id}/visuals/check`: guard, draw and look at a spec (Nary's vision check, AI
/// budget), nothing saved. {ok, notes, warnings, at, png_base64}.
pub async fn visual_check(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<VisualCheckIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let mut block_type = figure_type(body.block_type.as_deref())?.map(str::to_string);
    let mut size = body.size.clone();
    if let Some(bid) = body.block_id.as_deref().filter(|b| !b.is_empty()) {
        let id = router_uuid(bid)?;
        if let Some(b) = core::blocks_of(&mut conn, d.id)
            .await?
            .into_iter()
            .find(|b| b.id == id)
        {
            if granary_services::documents::figures::is_figure(&b.block_type) {
                block_type.get_or_insert(b.block_type.clone());
                if size.is_none() {
                    size = b
                        .data
                        .get("size")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
        }
    }
    let brief = body
        .brief
        .clone()
        .filter(|b| !b.trim().is_empty())
        .or(body.caption.clone())
        .unwrap_or_default();
    let spec = Value::Object(body.spec);
    drop(conn);
    run_job(
        &db,
        &user,
        &d,
        "visual_check",
        job(move |conn, d| {
            Box::pin(async move {
                let out = granary_services::nary::proposal_visuals::check_spec(
                    conn,
                    &d,
                    block_type.as_deref(),
                    &spec,
                    &brief,
                    figure_size(size.as_deref()),
                )
                .await?;
                commit(conn).await?;
                Ok((200, out))
            })
        }),
    )
    .await
}

/// `POST /documents/{doc_id}/visuals/{block_id}/redraw`: guard, draw and check the figure again (Nary fixes
/// it when the check finds problems). {block, ok, problems, checked, document}.
pub async fn visual_redraw(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((doc_id, block_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let bid = router_uuid(&block_id)?;
    drop(conn);
    run_job(
        &db,
        &user,
        &d,
        "visual_redraw",
        job(move |conn, d| {
            Box::pin(async move {
                let mut res =
                    granary_services::nary::proposal_visuals::redraw(conn, &d, bid).await?;
                commit(conn).await?;
                let d = reload(conn, d.id).await?;
                res["document"] = out(conn, &d, true).await?;
                Ok((200, res))
            })
        }),
    )
    .await
}

/// `POST /documents/{doc_id}/visuals/{block_id}/ask` ({text}): Nary changes that figure as asked (through
/// the guards and the check). {block, ok, problems, checked, document}.
pub async fn visual_ask(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((doc_id, block_id)): Path<(String, String)>,
    PyJson(body): PyJson<AskIn>,
) -> ApiResult<Response> {
    require(&user, &["sales:create_quotation"])?;
    let mut conn = db.conn().await?;
    let d = doc(&mut conn, &doc_id).await?;
    let bid = router_uuid(&block_id)?;
    drop(conn);
    let text = body.text;
    run_job(
        &db,
        &user,
        &d,
        "visual_ask",
        job(move |conn, d| {
            Box::pin(async move {
                let mut res = granary_services::nary::proposal_visuals::change_by_instruction(
                    conn, &d, bid, &text,
                )
                .await?;
                commit(conn).await?;
                let d = reload(conn, d.id).await?;
                res["document"] = out(conn, &d, true).await?;
                Ok((200, res))
            })
        }),
    )
    .await
}

// ------------------------------------------------------------------------------------- fix-longapply

/// One long job's work: on its own session (a transaction from the pool), the document as loaded.
type JobWork = Box<
    dyn for<'c> FnOnce(
            &'c mut PgConnection,
            SalesDocument,
        ) -> futures::future::BoxFuture<'c, ApiResult<(u16, Value)>>
        + Send,
>;

/// The closure of a [`JobWork`] (its signature inferred for every lifetime).
fn job<F>(f: F) -> JobWork
where
    F: for<'c> FnOnce(
            &'c mut PgConnection,
            SalesDocument,
        ) -> futures::future::BoxFuture<'c, ApiResult<(u16, Value)>>
        + Send
        + 'static,
{
    Box::new(f)
}

/// fix-longapply: a long proposal job (Opus, figures, vision checks) runs on a task of its own,
/// recorded in sales_document_jobs. The request waits `nary.apply_wait_seconds`: done in time, the answer is
/// exactly as before; else 202 {status: working, job_id, still_working} and the editor polls
/// GET /documents/{id}/jobs/{job_id}.
async fn run_job(
    db: &Db,
    user: &AuthUser,
    d: &SalesDocument,
    kind: &str,
    work: JobWork,
) -> ApiResult<Response> {
    use granary_services::nary::long_work::{self, Waited};
    let job_id = Uuid::new_v4();
    let wait = {
        let mut conn = db.conn().await?;
        let wait = long_work::wait_limit(&mut conn).await?;
        sqlx::query(
            "INSERT INTO sales_document_jobs (id, document_id, kind, status, created_by_id) \
             VALUES ($1, $2, $3, 'running', $4)",
        )
        .bind(job_id)
        .bind(d.id)
        .bind(kind)
        .bind(user.id)
        .execute(&mut *conn)
        .await?;
        wait
    };
    db.commit().await?;
    let doc = d.clone();
    let task = async move {
        let pool = granary_services::nary::session_pool()?;
        let outcome: ApiResult<(u16, Value)> = async {
            let mut tx = pool.begin().await?;
            let r = work(&mut tx, doc).await?;
            tx.commit().await?;
            Ok(r)
        }
        .await;
        let (status, http, result, error) = match &outcome {
            Ok((st, body)) => ("done", Some(*st as i32), Some(body.clone()), None),
            Err(e) => (
                "failed",
                granary_services::nary::governance::http_status(e).map(|s| s as i32),
                None,
                Some(
                    granary_services::nary::governance::http_detail(e)
                        .unwrap_or_else(|| e.to_string()),
                ),
            ),
        };
        let saved = sqlx::query(
            "UPDATE sales_document_jobs SET status = $2, http_status = $3, result = $4, error = $5, \
             finished_at = now(), updated_at = now() WHERE id = $1",
        )
        .bind(job_id)
        .bind(status)
        .bind(http)
        .bind(result)
        .bind(error)
        .execute(&pool)
        .await;
        if let Err(e) = saved {
            tracing::warn!(job_id = %job_id, error = %e, "sales_document_job_not_saved");
        }
        outcome
    };
    match long_work::wait_or_detach(wait, task).await {
        Waited::Done(Ok((st, body))) => Ok(resp::json_status(st, body)),
        Waited::Done(Err(e)) => Err(e),
        Waited::Working => Ok(resp::json_status(
            202,
            json!({"status": "working", "job_id": job_id.to_string(), "document_id": d.id.to_string(),
                   "kind": kind, "still_working": true, "label": long_work::STILL_WORKING_DOC}),
        )),
    }
}

/// `GET /documents/{doc_id}/jobs/{job_id}`: a long job's state {id, kind, status running | done | failed,
/// http_status, result (the answer the request would have given), error}.
pub async fn get_job(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((doc_id, job_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require(&user, &["sales:view_quotations", "sales:create_quotation"])?;
    let (did, jid) = (router_uuid(&doc_id)?, router_uuid(&job_id)?);
    let mut conn = db.conn().await?;
    type Row = (
        Uuid,
        String,
        String,
        Option<i32>,
        Option<Value>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, kind, status, http_status, result, error, created_at, finished_at \
         FROM sales_document_jobs WHERE id = $1 AND document_id = $2",
    )
    .bind(jid)
    .bind(did)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, kind, status, http, result, error, created, finished)) = row else {
        return Err(ApiError::http(404, "Job not found"));
    };
    Ok(resp::json(json!({
        "id": id.to_string(), "kind": kind, "status": status, "http_status": http, "result": result,
        "error": error, "created_at": iso(&created), "finished_at": iso_opt(finished.as_ref()),
    })))
}

granary_core::py_model! {
    /// fix-longapply: the body of `POST /documents/{doc_id}/reject`.
    pub struct RejectIn {
        pub reason: String,
    }
}

async fn may_reject(conn: &mut PgConnection, user: &AuthUser) -> ApiResult<()> {
    if granary_services::nary::proposal_lifecycle::may_reject(conn, user).await? {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        "Permission required: sales:create_quotation",
    ))
}

/// fix-longapply: `POST /documents/{doc_id}/reject` ({reason}): a draft or in-review proposal is rejected
/// (read-only, the reason kept, Nary's waiting cards about it withdrawn).
pub async fn reject(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<RejectIn>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    may_reject(&mut conn, &user).await?;
    let d = doc(&mut conn, &doc_id).await?;
    let d = granary_services::nary::proposal_lifecycle::reject(
        &mut conn,
        &d,
        &user,
        &body.reason,
        None,
    )
    .await?;
    commit(&mut conn).await?;
    Ok(resp::json(out(&mut conn, &d, true).await?))
}

/// fix-longapply: `POST /documents/{doc_id}/reopen`: a rejected proposal back to draft (a mistake).
pub async fn reopen(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    may_reject(&mut conn, &user).await?;
    let d = doc(&mut conn, &doc_id).await?;
    let d = granary_services::nary::proposal_lifecycle::reopen(&mut conn, &d).await?;
    commit(&mut conn).await?;
    Ok(resp::json(out(&mut conn, &d, true).await?))
}
