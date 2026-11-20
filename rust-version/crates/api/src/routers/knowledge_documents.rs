//! doclib A1: the document library API (Rust only), under `/api/v1/knowledge` (merged into the knowledge
//! router, so no new mount).
//!
//!   GET    /documents                          list (view, category, audience, language, product_id, q, paging)
//!   POST   /documents                          multipart meta (JSON) + file [+ source]: document + first version
//!   GET    /documents/{id}                     detail (+ description, articles, versions for doc admins, permissions)
//!   PATCH  /documents/{id}                     title, description, category, audiences, language, language group,
//!                                              products, review_by; owner (doc admins)
//!   DELETE /documents/{id}                     soft delete (documents:manage_all)
//!   GET    /documents/{id}/suggest-label       {label}
//!   POST   /documents/{id}/versions            multipart label, change_note, file [+ source]
//!   POST   /documents/{id}/versions/{vid}/restore     doc admins: a new version from an old one
//!   POST   /documents/{id}/versions/{vid}/approve     directors: approve the pending version ({comment})
//!   POST   /documents/{id}/versions/{vid}/reject      directors: reject it ({comment} = the reason)
//!   GET    /document-categories                every category (+ visible_roles)
//!   PUT    /document-categories                [{id|key, visible_roles}] (doc admins)
//!   GET    /articles/{article_id}/documents    the documents an article embeds (that the user can see)
//!   PUT    /articles/{article_id}/documents    {document_ids} (kb:create)
//!   GET    /api/v1/sales/catalog/{product_id}/documents   library documents of a product or its family
//!
//! The copies (view, download, trace, price list, partner portal) are A2's: knowledge_documents_copies.rs,
//! merged below.

use std::collections::HashMap;

use axum::extract::{Path, Query};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::approvals::engine;
use granary_services::doc_library::versions::{self, NewDocument, NewFile, NewVersion, ProductRef};
use granary_services::doc_library::{self as lib, access, view, Filters, Scope};

use super::at;
use crate::extract::PyForm;
use crate::resp;

/// The prefix of the library routes.
pub const PREFIX: &str = "/api/v1/knowledge";
const SALES_PREFIX: &str = "/api/v1/sales";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/documents"),
            get(list_documents).post(create_document),
        )
        .route(
            &at(PREFIX, "/documents/{id}"),
            get(get_document)
                .patch(update_document)
                .delete(delete_document),
        )
        .route(
            &at(PREFIX, "/documents/{id}/suggest-label"),
            get(suggest_label),
        )
        .route(
            &at(PREFIX, "/documents/{id}/versions"),
            post(upload_version),
        )
        .route(
            &at(PREFIX, "/documents/{id}/versions/{vid}/restore"),
            post(restore_version),
        )
        .route(
            &at(PREFIX, "/documents/{id}/versions/{vid}/approve"),
            post(approve_version),
        )
        .route(
            &at(PREFIX, "/documents/{id}/versions/{vid}/reject"),
            post(reject_version),
        )
        .route(
            &at(PREFIX, "/document-categories"),
            get(list_categories).put(save_categories),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}/documents"),
            get(article_documents).put(set_article_documents),
        )
        .route(
            &at(SALES_PREFIX, "/catalog/{product_id}/documents"),
            get(catalog_documents),
        )
        .merge(super::knowledge_documents_copies::router())
}

fn uuid_of(s: &str, what: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(s.trim()).map_err(|_| ApiError::http(404, format!("{what} not found")))
}

fn require_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains(access::PERM_VIEW) || access::is_doc_admin(user) {
        Ok(())
    } else {
        Err(ApiError::http(403, "Permission required: documents:view"))
    }
}

fn file_of(f: crate::extract::UploadFile) -> NewFile {
    NewFile {
        data: f.data.to_vec(),
        file_name: f.filename,
        mime: f.content_type,
    }
}

/// `GET /documents`.
pub async fn list_documents(
    InternalUser(user): InternalUser,
    db: Db,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let s = |k: &str| {
        q.get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let num = |k: &str| s(k).and_then(|v| v.parse::<i64>().ok());
    // ?version=<id> (approval links point at a version): only the document it belongs to, when visible
    if let Some(v) = s("version") {
        let vid =
            Uuid::parse_str(&v).map_err(|_| ApiError::http(400, "version is not a valid id"))?;
        let mut conn = db.conn().await?;
        let mut items = Vec::new();
        if let Some(ver) = lib::load_version(&mut conn, vid).await? {
            if let Some(d) = lib::load_document(&mut conn, ver.document_id).await? {
                if !d.is_deleted && access::can_view(&mut conn, &user, &d).await? {
                    items.push(view::item(&mut conn, Some(&user), &d).await?);
                }
            }
        }
        let total = items.len();
        return Ok(resp::json(json!({"items": items, "total": total})));
    }
    let product_id = match s("product_id") {
        Some(p) => Some(
            Uuid::parse_str(&p).map_err(|_| ApiError::http(400, "product_id is not a valid id"))?,
        ),
        None => None,
    };
    let filters = Filters {
        view: s("view"),
        category: s("category"),
        audience: s("audience"),
        language: s("language"),
        product_id,
        family: s("family"),
        limit: num("limit"),
        offset: num("offset"),
        only_live: false,
        include_price_list: true,
    };
    let mut conn = db.conn().await?;
    let found = lib::search(&mut conn, &Scope::Staff(&user), s("q").as_deref(), &filters).await?;
    let mut items = Vec::with_capacity(found.items.len());
    for d in &found.items {
        items.push(view::item(&mut conn, Some(&user), d).await?);
    }
    Ok(resp::json(json!({"items": items, "total": found.total})))
}

/// `POST /documents` (multipart: meta JSON, file, optional source).
pub async fn create_document(
    InternalUser(user): InternalUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = granary_core::validation::Cx::new();
    let meta = form.req_text(&mut cx, "meta");
    let file = form.req_file(&mut cx, "file");
    let source = form.opt_file(&mut cx, "source");
    form.check(cx)?;
    let meta: NewDocument = serde_json::from_str(&meta.unwrap_or_default())
        .map_err(|e| ApiError::http(422, format!("meta is not valid: {e}")))?;
    let mut conn = db.conn().await?;
    let (doc, _) = versions::create_document(
        &mut conn,
        &user,
        meta,
        file_of(file.unwrap()),
        source.map(file_of),
    )
    .await?;
    let out = view::detail(&mut conn, &user, &doc).await?;
    Ok(resp::json_status(201, out))
}

/// `GET /documents/{id}`.
pub async fn get_document(
    Path(id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    require_view(&user)?;
    let id = uuid_of(&id, "Document")?;
    let mut conn = db.conn().await?;
    let doc = access::viewable(&mut conn, &user, id).await?;
    Ok(resp::json(view::detail(&mut conn, &user, &doc).await?))
}

/// `PATCH /documents/{id}`.
pub async fn update_document(
    Path(id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let id = uuid_of(&id, "Document")?;
    let Some(b) = body.as_object() else {
        return Err(ApiError::http(
            422,
            "Send the fields to change as a JSON object",
        ));
    };
    let mut conn = db.conn().await?;
    // a document admin may change the owner without the right to edit the document itself
    let only_owner = b.keys().all(|k| k == "owner_id");
    let doc = if only_owner && access::is_doc_admin(&user) {
        access::viewable(&mut conn, &user, id).await?
    } else {
        access::editable(&mut conn, &user, id).await?
    };
    let has = |k: &str| b.contains_key(k);
    let text = |k: &str| b.get(k).and_then(Value::as_str).map(str::trim);

    if has("title") {
        let t: String = text("title").unwrap_or("").chars().take(300).collect();
        if t.is_empty() {
            return Err(ApiError::http(400, "A document needs a title"));
        }
        sqlx::query("UPDATE library_documents SET title = $2 WHERE id = $1")
            .bind(id)
            .bind(t)
            .execute(&mut *conn)
            .await?;
    }
    if has("description") {
        sqlx::query("UPDATE library_documents SET description = $2 WHERE id = $1")
            .bind(id)
            .bind(text("description").filter(|s| !s.is_empty()))
            .execute(&mut *conn)
            .await?;
    }
    if has("category") || has("category_id") {
        let c = text("category").or(text("category_id")).unwrap_or("");
        let cid = versions::category_id(&mut conn, c).await?;
        if !access::can_view_category(&mut conn, &user, cid).await? {
            return Err(ApiError::http(403, "You cannot see this category"));
        }
        sqlx::query("UPDATE library_documents SET category_id = $2 WHERE id = $1")
            .bind(id)
            .bind(cid)
            .execute(&mut *conn)
            .await?;
    }
    if has("audiences") {
        let aud: Vec<String> = serde_json::from_value(b["audiences"].clone())
            .map_err(|_| ApiError::http(422, "audiences is a list"))?;
        let mut clean: Vec<String> = Vec::new();
        for a in aud {
            let a = a.trim().to_lowercase();
            if !lib::AUDIENCES.contains(&a.as_str()) {
                return Err(ApiError::http(
                    400,
                    format!("Unknown audience '{a}': use internal, customer or partner"),
                ));
            }
            if !clean.contains(&a) {
                clean.push(a);
            }
        }
        if clean.is_empty() {
            return Err(ApiError::http(
                400,
                "A document needs at least one audience",
            ));
        }
        clean.sort_by_key(|a| lib::AUDIENCES.iter().position(|x| x == a));
        // a document that starts going to customers or partners: only a director may widen it
        let widening = !doc.is_external() && clean.iter().any(|a| a != "internal");
        if widening && !access::can_approve(&user) {
            return Err(ApiError::http(
                403,
                "Only a director can make an internal document customer- or partner-facing (its version was never approved)",
            ));
        }
        if doc.kind == "price_list" && clean.iter().any(|a| a != "internal") {
            return Err(ApiError::http(400, "The price list stays internal"));
        }
        sqlx::query("UPDATE library_documents SET audiences = $2 WHERE id = $1")
            .bind(id)
            .bind(&clean)
            .execute(&mut *conn)
            .await?;
    }
    if has("language") {
        let l = versions::clean_language(text("language"))?;
        sqlx::query("UPDATE library_documents SET language = $2 WHERE id = $1")
            .bind(id)
            .bind(l)
            .execute(&mut *conn)
            .await?;
    }
    for key in ["language_group_document_id", "variant_of"] {
        if !has(key) {
            continue;
        }
        let group = match text(key).filter(|s| !s.is_empty()) {
            None => None,
            Some(other) => {
                let other = Uuid::parse_str(other)
                    .map_err(|_| ApiError::http(400, "The other language version was not found"))?;
                if other == id {
                    return Err(ApiError::http(
                        400,
                        "A document cannot be its own language version",
                    ));
                }
                Some(versions::group_with(&mut conn, other).await?)
            }
        };
        sqlx::query("UPDATE library_documents SET language_group_id = $2 WHERE id = $1")
            .bind(id)
            .bind(group)
            .execute(&mut *conn)
            .await?;
    }
    if has("products") {
        let products: Vec<ProductRef> = serde_json::from_value(b["products"].clone())
            .map_err(|_| ApiError::http(422, "products is a list of {product_id} or {family}"))?;
        versions::set_products(&mut conn, id, &products).await?;
    }
    if has("review_by") {
        let r = match text("review_by").filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => Some(
                chrono::NaiveDate::parse_from_str(&s[..s.len().min(10)], "%Y-%m-%d")
                    .map_err(|_| ApiError::http(422, "review_by is a date (YYYY-MM-DD)"))?,
            ),
        };
        sqlx::query(
            "UPDATE library_documents SET review_by = $2, last_reminded_at = NULL WHERE id = $1",
        )
        .bind(id)
        .bind(r)
        .execute(&mut *conn)
        .await?;
    }
    if has("owner_id") {
        access::require_doc_admin(&user)?;
        let o = text("owner_id")
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| ApiError::http(400, "owner_id is a user id"))?;
        let ok: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id = $1 AND is_active = true \
             AND (portal_type = 'internal' OR portal_type IS NULL OR portal_type = '')",
        )
        .bind(o)
        .fetch_optional(&mut *conn)
        .await?;
        if ok.is_none() {
            return Err(ApiError::http(
                400,
                "The new owner must be an active staff member",
            ));
        }
        sqlx::query("UPDATE library_documents SET owner_id = $2 WHERE id = $1")
            .bind(id)
            .bind(o)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("UPDATE library_documents SET updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let doc = access::viewable(&mut conn, &user, id).await?;
    Ok(resp::json(view::detail(&mut conn, &user, &doc).await?))
}

/// `DELETE /documents/{id}` (soft; directors).
pub async fn delete_document(
    Path(id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let id = uuid_of(&id, "Document")?;
    let mut conn = db.conn().await?;
    access::viewable(&mut conn, &user, id).await?;
    if !access::is_manager_all(&user) {
        return Err(ApiError::http(403, "Only a director can delete a document"));
    }
    // a pending approval stops counting
    for v in lib::versions_of(&mut conn, id).await? {
        if v.status == lib::STATUS_PENDING {
            engine::supersede_for_entity(
                &mut conn,
                versions::ENTITY_TYPE,
                v.id,
                "Document deleted",
            )
            .await?;
            versions::mark_rejected(&mut conn, v.id, Some("Document deleted")).await?;
        }
    }
    sqlx::query("UPDATE library_documents SET is_deleted = true, updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(resp::no_content())
}

/// `GET /documents/{id}/suggest-label`.
pub async fn suggest_label(
    Path(id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let id = uuid_of(&id, "Document")?;
    let mut conn = db.conn().await?;
    access::viewable(&mut conn, &user, id).await?;
    let label = lib::next_label(&mut conn, id).await?;
    Ok(resp::json(json!({"label": label})))
}

/// `POST /documents/{id}/versions` (multipart: label, change_note, file, optional source).
pub async fn upload_version(
    Path(id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let id = uuid_of(&id, "Document")?;
    let mut cx = granary_core::validation::Cx::new();
    let change_note = form.req_text(&mut cx, "change_note");
    let file = form.req_file(&mut cx, "file");
    let source = form.opt_file(&mut cx, "source");
    form.check(cx)?;
    let mut conn = db.conn().await?;
    let doc = access::viewable(&mut conn, &user, id).await?;
    if doc.kind == "price_list" {
        return Err(ApiError::http(
            400,
            "The price list is generated from the catalog; use Generate price list",
        ));
    }
    let v = versions::add_version(
        &mut conn,
        &user,
        &doc,
        NewVersion {
            label: form.opt_text("label"),
            change_note: change_note.unwrap_or_default(),
            file: file_of(file.unwrap()),
            source: source.map(file_of),
        },
    )
    .await?;
    Ok(resp::json_status(
        201,
        view::version_full(&mut conn, &v).await?,
    ))
}

/// `POST /documents/{id}/versions/{vid}/restore` (doc admins).
pub async fn restore_version(
    Path((id, vid)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let id = uuid_of(&id, "Document")?;
    let vid = uuid_of(&vid, "Version")?;
    let mut conn = db.conn().await?;
    let doc = access::viewable(&mut conn, &user, id).await?;
    let v = versions::restore(&mut conn, &user, &doc, vid).await?;
    Ok(resp::json_status(
        201,
        view::version_full(&mut conn, &v).await?,
    ))
}

async fn decide(
    id: &str,
    vid: &str,
    user: &AuthUser,
    db: &Db,
    decision: &str,
    comment: Option<String>,
) -> ApiResult<Response> {
    let id = uuid_of(id, "Document")?;
    let vid = uuid_of(vid, "Version")?;
    let mut conn = db.conn().await?;
    access::viewable(&mut conn, user, id).await?;
    if !access::can_approve(user) {
        return Err(ApiError::http(
            403,
            "Only a director can approve or reject a document version",
        ));
    }
    let v = lib::load_version(&mut conn, vid)
        .await?
        .filter(|v| v.document_id == id)
        .ok_or_else(|| ApiError::http(404, "Version not found"))?;
    if decision == "rejected" && comment.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(ApiError::http(400, "Say why the version is rejected"));
    }
    engine::act_on_pending(
        &mut conn,
        versions::ENTITY_TYPE,
        v.id,
        user,
        decision,
        comment.as_deref(),
    )
    .await?;
    let v = lib::load_version(&mut conn, vid).await?.unwrap_or(v);
    Ok(resp::json(view::version_full(&mut conn, &v).await?))
}

fn comment_of(body: Option<Json<Value>>) -> Option<String> {
    body.and_then(|Json(b)| {
        b.get("comment")
            .or_else(|| b.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

/// `POST /documents/{id}/versions/{vid}/approve`.
pub async fn approve_version(
    Path((id, vid)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
    body: Option<Json<Value>>,
) -> ApiResult<Response> {
    decide(&id, &vid, &user, &db, "approved", comment_of(body)).await
}

/// `POST /documents/{id}/versions/{vid}/reject` ({comment}: the reason).
pub async fn reject_version(
    Path((id, vid)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
    body: Option<Json<Value>>,
) -> ApiResult<Response> {
    decide(&id, &vid, &user, &db, "rejected", comment_of(body)).await
}

fn category_json(c: &lib::Category) -> Value {
    json!({
        "id": c.id, "key": c.key, "name": c.name, "sort_order": c.sort_order, "visible_roles": c.visible_roles,
    })
}

/// `GET /document-categories`: every category (a staff member who cannot see one still learns it exists).
pub async fn list_categories(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let cats = lib::categories(&mut conn).await?;
    let mut items = Vec::new();
    for c in &cats {
        let mut j = category_json(c);
        j["can_view"] = json!(access::can_view_category(&mut conn, &user, c.id).await?);
        items.push(j);
    }
    Ok(resp::json(json!({"items": items})))
}

/// `PUT /document-categories`: `[{id or key, visible_roles}]` (or `{items: [...]}`); doc admins.
pub async fn save_categories(
    InternalUser(user): InternalUser,
    db: Db,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    access::require_doc_admin(&user)?;
    let rows = match &body {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => vec![],
    };
    let mut conn = db.conn().await?;
    let slugs: Vec<String> = sqlx::query_scalar("SELECT slug FROM roles")
        .fetch_all(&mut *conn)
        .await?;
    for r in rows {
        let key = r
            .get("id")
            .or_else(|| r.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let cid = versions::category_id(&mut conn, key).await?;
        let roles: Vec<String> =
            serde_json::from_value(r.get("visible_roles").cloned().unwrap_or(json!([])))
                .map_err(|_| ApiError::http(422, "visible_roles is a list of role slugs"))?;
        let mut clean: Vec<String> = Vec::new();
        for s in roles {
            let s = s.trim().to_string();
            if !slugs.contains(&s) {
                return Err(ApiError::http(400, format!("Unknown role '{s}'")));
            }
            if !clean.contains(&s) {
                clean.push(s);
            }
        }
        sqlx::query(
            "UPDATE doc_categories SET visible_roles = $2, updated_at = now() WHERE id = $1",
        )
        .bind(cid)
        .bind(&clean)
        .execute(&mut *conn)
        .await?;
    }
    let cats = lib::categories(&mut conn).await?;
    Ok(resp::json(
        json!({"items": cats.iter().map(category_json).collect::<Vec<_>>()}),
    ))
}

/// `GET /articles/{article_id}/documents`: the linked documents the user can see.
pub async fn article_documents(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let aid = uuid_of(&article_id, "Article")?;
    let mut conn = db.conn().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT l.document_id FROM library_article_links l JOIN library_documents d ON d.id = l.document_id \
          WHERE l.article_id = $1 AND d.is_deleted = false ORDER BY l.created_at, d.title",
    )
    .bind(aid)
    .fetch_all(&mut *conn)
    .await?;
    let mut items = Vec::new();
    for id in ids {
        if let Some(d) = lib::load_document(&mut conn, id).await? {
            if access::can_view(&mut conn, &user, &d).await? {
                items.push(view::item(&mut conn, Some(&user), &d).await?);
            }
        }
    }
    Ok(resp::json(json!({"items": items})))
}

/// `PUT /articles/{article_id}/documents` `{document_ids}` (kb:create): replaces the links.
pub async fn set_article_documents(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    if !(user.is_superadmin || user.perms.contains("kb:create")) {
        return Err(ApiError::http(403, "Permission required: kb:create"));
    }
    let aid = uuid_of(&article_id, "Article")?;
    let ids: Vec<Uuid> =
        serde_json::from_value(body.get("document_ids").cloned().unwrap_or(json!([])))
            .map_err(|_| ApiError::http(422, "document_ids is a list of document ids"))?;
    let mut conn = db.conn().await?;
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM kb_articles WHERE id = $1 AND is_deleted = false")
            .bind(aid)
            .fetch_optional(&mut *conn)
            .await?;
    if exists.is_none() {
        return Err(ApiError::http(404, "Article not found"));
    }
    for id in &ids {
        access::viewable(&mut conn, &user, *id).await?;
    }
    sqlx::query("DELETE FROM library_article_links WHERE article_id = $1")
        .bind(aid)
        .execute(&mut *conn)
        .await?;
    for id in &ids {
        sqlx::query(
            "INSERT INTO library_article_links (article_id, document_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(aid)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    article_documents(Path(article_id), InternalUser(user), db).await
}

/// `GET /api/v1/sales/catalog/{product_id}/documents`: the library documents of a product or its family
/// that the user can see.
pub async fn catalog_documents(
    Path(product_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    require_view(&user)?;
    let pid = uuid_of(&product_id, "Product")?;
    let filters = Filters {
        product_id: Some(pid),
        limit: Some(200),
        include_price_list: false,
        ..Filters::default()
    };
    let mut conn = db.conn().await?;
    let found = lib::search(&mut conn, &Scope::Staff(&user), None, &filters).await?;
    let mut items = Vec::new();
    for d in &found.items {
        items.push(view::item(&mut conn, Some(&user), d).await?);
    }
    Ok(resp::json(json!({"items": items, "total": found.total})))
}
