//! backend/app/routers/knowledge.py, mounted at `/api/v1/kb` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: every route takes `InternalUser` itself.
//!
//! `_open_revision` / `_create_revision` live in granary_services::later::nary_sop_revisions (the services
//! crate calls them too and cannot depend on this crate).
//!
//! Starlette route order: `POST /articles/import-markdown` is registered after the `/articles/{article_id}`
//! routes (GET, PATCH, DELETE), so `GET /articles/import-markdown` is `get_article("import-markdown")`
//! (an article with that slug). axum would answer 405 on the static path: those methods are added to it.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{v, Cx, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::attachment::Attachment;
use granary_db::rows::knowledge::{KBArticle, KBArticleRevision, KBCategory};
use granary_services::approvals::adapters::{publish_sop_article, Entity, SOP_SIGNATURE_FIELDS};
use granary_services::approvals::{engine, notify};
use granary_services::later::nary_sop_revisions::{_create_revision, _open_revision};

use super::at;
use super::projects::{
    bind_val, has_perm, now_utc, py_int, py_str, truthy, update_row, uuid_opt, Bound, Kind, Val,
};
use crate::extract::PyForm;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/kb";

/// The routes of `router` in knowledge.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        // doclib A1: the document library (/api/v1/knowledge/..., /api/v1/sales/catalog/{id}/documents)
        .merge(super::knowledge_documents::router())
        .route(
            &at(PREFIX, "/categories"),
            get(list_categories).post(create_category),
        )
        .route(
            &at(PREFIX, "/articles"),
            get(list_articles).post(create_article),
        )
        .route(
            &at(PREFIX, "/articles/import-markdown"),
            post(import_markdown)
                .get(|u: InternalUser, db: Db| get_article(Path("import-markdown".into()), u, db))
                .patch(
                    |u: InternalUser, db: Db, b: PyJson<IndexMap<String, PyObj>>| {
                        update_article(Path("import-markdown".into()), u, db, b)
                    },
                )
                .delete(|u: InternalUser, db: Db| {
                    delete_article(Path("import-markdown".into()), u, db)
                }),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}"),
            get(get_article)
                .patch(update_article)
                .delete(delete_article),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}/publish"),
            patch(publish_article),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}/revisions"),
            get(get_article_revisions),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}/sign"),
            post(sign_article),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}/sign/owner"),
            post(sign_article_owner),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}/pdf"),
            get(export_article_pdf),
        )
        .route(
            &at(PREFIX, "/articles/{article_id}/import-pdf"),
            post(import_pdf),
        )
}

/// `_queue_reindex(article_id)`: best effort, re-embed this article for AI retrieval.
async fn _queue_reindex(article_id: Uuid) {
    let _ = granary_services::tasks::delay(
        "reindex_kb_embeddings",
        vec![json!(article_id.to_string())],
        Map::new(),
    )
    .await;
}

/// `_queue_pdf_import(article_id)`: hand the conversion to the background worker; the error message when the
/// job could not be queued (a silently dropped job leaves the article 'pending' forever).
async fn _queue_pdf_import(article_id: Uuid) -> Option<String> {
    // A queued job nothing consumes is the most common cause of a stuck import. Say so now.
    if !granary_services::tasks::ping(std::time::Duration::from_millis(1500)).await {
        return Some(
            "No background worker responded, so the conversion cannot start. \
             Check that the background worker is running and up to date."
                .into(),
        );
    }
    match granary_services::tasks::delay(
        "import_kb_article_pdf",
        vec![json!(article_id.to_string())],
        Map::new(),
    )
    .await
    {
        Ok(_) => None,
        Err(e) => Some(format!(
            "Could not queue the conversion: {}",
            e.to_string().chars().take(200).collect::<String>()
        )),
    }
}

/// `_pdf_import_dict(db, article)`: conversion state + the source PDF, for the article detail.
async fn _pdf_import_dict(
    db: &mut PgConnection,
    article: &KBArticle,
) -> ApiResult<Vec<(&'static str, Value)>> {
    let mut out = vec![
        ("pdf_import_status", json!(article.pdf_import_status)),
        ("pdf_import_error", json!(article.pdf_import_error)),
        ("pdf_import_mode", json!(article.pdf_import_mode)),
        ("pdf_import_pages", json!(article.pdf_import_pages)),
        (
            "pdf_import_truncated",
            json!(article.pdf_import_truncated.unwrap_or(false)),
        ),
        ("source_pdf", Value::Null),
    ];
    let Some(att_id) = article.source_pdf_attachment_id else {
        return Ok(out);
    };
    let att: Option<Attachment> =
        sqlx::query_as("SELECT * FROM attachments WHERE attachments.id = $1")
            .bind(att_id)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(att) = att {
        out[5].1 = json!({"id": att.id.to_string(), "file_name": att.file_name, "file_size": att.file_size});
    }
    Ok(out)
}

/// `_slugify(text)`.
fn _slugify(text: &str) -> String {
    let lower = pystr::lower(text);
    // re.sub(r"[^\w\s-]", "", text)
    let kept: String = lower
        .chars()
        .filter(|c| pystr::is_word(*c) || pystr::is_space(*c) || *c == '-')
        .collect();
    // re.sub(r"[-\s]+", "-", text)
    let mut out = String::new();
    let mut in_run = false;
    for c in kept.chars() {
        if c == '-' || pystr::is_space(c) {
            if !in_run {
                out.push('-');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out.trim_matches('-').chars().take(80).collect()
}

/// A frontmatter value: a string, or a `[a, b]` list.
#[derive(Clone, Debug)]
enum Meta {
    Str(String),
    List(Vec<String>),
}

/// `_parse_frontmatter(content)`: (meta, body). Handles --- YAML-ish blocks.
fn _parse_frontmatter(content: &str) -> (IndexMap<String, Meta>, String) {
    if !content.starts_with("---") {
        return (IndexMap::new(), content.to_string());
    }
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        return (IndexMap::new(), content.to_string());
    }
    let strip_q = |s: &str| s.trim_matches(|c| c == '"' || c == '\'').to_string();
    let mut meta = IndexMap::new();
    for line in pystr::splitlines(pystr::strip(parts[1])) {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = strip_q(pystr::strip(val));
        let key = pystr::strip(key).to_string();
        if val.starts_with('[') && val.ends_with(']') && val.chars().count() >= 2 {
            let inner: String = {
                let n = val.chars().count();
                val.chars().skip(1).take(n - 2).collect()
            };
            let items = inner
                .split(',')
                .filter(|v| !pystr::strip(v).is_empty())
                .map(|v| strip_q(pystr::strip(v)))
                .collect();
            meta.insert(key, Meta::List(items));
        } else {
            meta.insert(key, Meta::Str(val));
        }
    }
    (meta, pystr::strip(parts[2]).to_string())
}

/// `_check_sop_permission(current_user, article_type)`: 403 unless the user may manage SOPs.
fn _check_sop_permission(user: &AuthUser, article_type: Option<&str>) -> ApiResult<()> {
    if article_type != Some("sop") {
        return Ok(());
    }
    if !user.is_superadmin && !has_perm(user, "kb:manage_sops") {
        return Err(ApiError::http(
            403,
            "Permission required: kb:manage_sops (executive management only)",
        ));
    }
    Ok(())
}

granary_core::py_model! {
    /// `class CategoryCreate(BaseModel)`.
    pub struct CategoryCreate {
        pub name: String,
        pub slug: String,
        pub description: Option<String> = None,
        pub parent_id: Option<String> = None,
        /// all_staff | department | management | restricted
        pub access_level: String = "all_staff",
        pub icon: Option<String> = None,
        /// Where the category sits on the categories page (smaller first).
        pub sort_order: i64 = 0
            => v::int_c(v::NumC::new().ge("-100000").le("100000")),
    }
}

granary_core::py_model! {
    /// `class ArticleCreate(BaseModel)`.
    pub struct ArticleCreate {
        pub title: String,
        /// auto-generated from title when omitted
        pub slug: Option<String> = None,
        pub category_id: Option<String> = None,
        /// wiki | sop | runbook | guide | policy | template | faq
        pub article_type: String = "wiki",
        pub content: String = "",
        pub summary: Option<String> = None,
        pub sop_code: Option<String> = None,
        pub sop_steps: Option<Vec<serde_json::Map<String, Value>>> = None,
        pub sop_trigger: Option<String> = None,
        pub sop_owner_role: Option<String> = None,
        pub tags: Vec<String> = Vec::new(),
        pub access_level: String = "all_staff",
        pub department: Option<String> = None,
        pub is_private: bool = false,
    }
}

/// `GET "/categories"` (internal users only).
pub async fn list_categories(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    if !has_perm(&user, "kb:manage_categories") {
        _require_kb_view(&user)?;
    }
    let mut conn = db.conn().await?;
    let cats: Vec<KBCategory> = sqlx::query_as(
        "SELECT * FROM kb_categories WHERE kb_categories.is_deleted = false ORDER BY kb_categories.name",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = cats
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(), "name": c.name, "slug": c.slug,
                // The categories page shows the description and orders by sort_order.
                "description": c.description, "sort_order": c.sort_order,
                "access_level": c.access_level,
                "parent_id": uuid_opt(c.parent_id),
                "icon": c.icon,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST "/categories"` (201).
pub async fn create_category(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CategoryCreate>,
) -> ApiResult<Response> {
    if !has_perm(&user, "kb:manage_categories") && !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "Permission required: kb:manage_categories",
        ));
    }
    let parent_id = pg_uuid_opt(body.parent_id.as_deref())?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    // KBCategory(**body.model_dump()): no created_by_id column
    sqlx::query(
        "INSERT INTO kb_categories (id, name, slug, description, parent_id, access_level, icon, sort_order, \
         allowed_departments, allowed_roles, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '{}', '{}', false)",
    )
    .bind(id)
    .bind(&body.name)
    .bind(&body.slug)
    .bind(&body.description)
    .bind(parent_id)
    .bind(&body.access_level)
    .bind(&body.icon)
    .bind(body.sort_order as i32)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

/// Access levels everyone may read; anything else (management, restricted) needs kb:view_restricted.
const OPEN_ACCESS_LEVELS: &[&str] = &["all_staff", "department"];

/// Batch E: reading the knowledge base needs kb:view (seeded on every staff role), like the other modules'
/// `:view` permissions. The editing permissions include reading, so an editor without kb:view still sees
/// what they write.
const KB_READ_PERMS: &[&str] = &["kb:view", "kb:create", "kb:publish", "kb:manage_sops"];

fn _can_view_kb(user: &AuthUser) -> bool {
    user.is_superadmin || KB_READ_PERMS.iter().any(|p| has_perm(user, p))
}

fn _require_kb_view(user: &AuthUser) -> ApiResult<()> {
    if _can_view_kb(user) {
        Ok(())
    } else {
        Err(ApiError::http(403, "Permission required: kb:view"))
    }
}

fn _can_edit_kb(user: &AuthUser) -> bool {
    user.is_superadmin || has_perm(user, "kb:create")
}

/// `_can_read_article(user, article)`: private ones only for their creator, management/restricted ones
/// only with kb:view_restricted, unpublished ones only for editors.
fn _can_read_article(user: &AuthUser, article: &KBArticle) -> bool {
    if user.is_superadmin {
        return true;
    }
    if !_can_view_kb(user) {
        return false;
    }
    if article.is_private && article.created_by_id != Some(user.id) {
        return false;
    }
    let open = article
        .access_level
        .as_deref()
        .is_some_and(|l| OPEN_ACCESS_LEVELS.contains(&l));
    if !open && !has_perm(user, "kb:view_restricted") {
        return false;
    }
    if article.status.as_deref() != Some("published") && !_can_edit_kb(user) {
        return false;
    }
    true
}

granary_core::py_model! {
    /// Query of `GET "/articles"`.
    pub struct ListArticlesQ {
        pub category_id: Option<String> = None,
        pub article_type: Option<String> = None,
        pub status: Option<String> = None,
        pub q: Option<String> = None,
        pub limit: i64 = 500,
        pub offset: i64 = 0,
    }
}

/// `GET "/articles"`.
pub async fn list_articles(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ListArticlesQ>,
) -> ApiResult<Response> {
    _require_kb_view(&user)?;
    let can_edit = _can_edit_kb(&user);
    enum P {
        Text(String),
        Uuid(Uuid),
    }
    let mut params: Vec<P> = Vec::new();
    let mut sql = String::from("SELECT * FROM kb_articles WHERE kb_articles.is_deleted = false");
    match q.status.as_deref() {
        Some(s) if !s.is_empty() => {
            params.push(P::Text(s.to_string()));
            sql.push_str(&format!(" AND kb_articles.status = ${}", params.len()));
        }
        // A revision that replaced its published SOP is history, not a second article
        _ => sql.push_str(
            " AND (kb_articles.previous_version_id IS NULL OR kb_articles.status != 'archived')",
        ),
    }
    // A ?status= filter narrows what the user may see; it never lifts the published-only rule
    if !can_edit {
        sql.push_str(" AND kb_articles.status = 'published'");
    }
    if !(user.is_superadmin || has_perm(&user, "kb:view_restricted")) {
        sql.push_str(" AND kb_articles.access_level IN ('all_staff', 'department')");
    }
    // Private articles are visible only to their creator (and superadmins)
    if !user.is_superadmin {
        params.push(P::Uuid(user.id));
        sql.push_str(&format!(
            " AND (kb_articles.is_private = false OR kb_articles.created_by_id = ${})",
            params.len()
        ));
    }
    if let Some(c) = q.category_id.as_deref().filter(|c| !c.is_empty()) {
        params.push(P::Uuid(pg_uuid(c)?));
        sql.push_str(&format!(" AND kb_articles.category_id = ${}", params.len()));
    }
    if let Some(t) = q.article_type.as_deref().filter(|t| !t.is_empty()) {
        params.push(P::Text(t.to_string()));
        sql.push_str(&format!(
            " AND kb_articles.article_type = ${}",
            params.len()
        ));
    }
    if let Some(s) = q.q.as_deref().filter(|s| !s.is_empty()) {
        params.push(P::Text(format!(
            "%{}%",
            granary_compat::text::utils::escape_like(s)
        )));
        sql.push_str(&format!(
            " AND kb_articles.title ILIKE ${} ESCAPE '\\'",
            params.len()
        ));
    }
    sql.push_str(&format!(
        " ORDER BY kb_articles.updated_at DESC LIMIT ${} OFFSET ${}",
        params.len() + 1,
        params.len() + 2
    ));
    let mut query = sqlx::query_as::<_, KBArticle>(&sql);
    for p in params {
        query = match p {
            P::Text(t) => query.bind(t),
            P::Uuid(u) => query.bind(u),
        };
    }
    let query = query.bind(q.limit).bind(q.offset);
    let mut conn = db.conn().await?;
    let articles = query.fetch_all(&mut *conn).await?;
    let out: Vec<Value> = articles
        .iter()
        .map(|a| {
            json!({
                "id": a.id.to_string(), "title": a.title, "slug": a.slug, "article_type": a.article_type,
                "sop_code": a.sop_code, "summary": a.summary, "tags": a.tags,
                "access_level": a.access_level, "version": a.version, "status": a.status,
                "is_private": a.is_private,
                "created_by_id": uuid_opt(a.created_by_id),
                "category_id": uuid_opt(a.category_id),
                "updated_at": resp::iso(&a.updated_at),
                "created_at": resp::iso(&a.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `re.sub(r"[^a-z0-9]+", "-", s)`.
fn dash_non_alnum(s: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in s.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            in_run = false;
        } else {
            if !in_run {
                out.push('-');
            }
            in_run = true;
        }
    }
    out
}

/// `POST "/articles"` (201): a draft.
pub async fn create_article(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<ArticleCreate>,
) -> ApiResult<Response> {
    if !has_perm(&user, "kb:create") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: kb:create"));
    }
    _check_sop_permission(&user, Some(&body.article_type))?;
    let mut conn = db.conn().await?;

    // Slug: use the provided one, else derive from the title. Ensure uniqueness (slug is unique).
    let mut slug = pystr::strip(body.slug.as_deref().unwrap_or("")).to_string();
    if slug.is_empty() {
        let title = if body.title.is_empty() {
            "article"
        } else {
            &body.title
        };
        slug = dash_non_alnum(&pystr::lower(title))
            .trim_matches('-')
            .to_string();
        if slug.is_empty() {
            slug = "article".into();
        }
    }
    let base_slug = slug.clone();
    let mut n = 1;
    loop {
        let taken: Option<(Uuid,)> =
            sqlx::query_as("SELECT kb_articles.id FROM kb_articles WHERE kb_articles.slug = $1")
                .bind(&slug)
                .fetch_optional(&mut *conn)
                .await?;
        if taken.is_none() {
            break;
        }
        n += 1;
        slug = format!("{base_slug}-{n}");
    }
    let sop_code = {
        let c = pystr::strip(body.sop_code.as_deref().unwrap_or("")).to_string();
        (!c.is_empty()).then_some(c)
    };
    let sop_steps = match &body.sop_steps {
        Some(s) if !s.is_empty() => json!(s),
        _ => json!([]),
    };
    let category_id = match body.category_id.as_deref() {
        Some(c) if !c.is_empty() => Some(pg_uuid(c)?),
        _ => None,
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO kb_articles (id, title, slug, content, summary, article_type, sop_code, sop_steps, \
         sop_trigger, sop_owner_role, tags, access_level, is_private, category_id, status, version, \
         created_by_id, related_article_ids, view_count, helpful_votes, unhelpful_votes, \
         pdf_import_truncated, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, 'draft', '1.0', $15, \
         '{}', 0, 0, 0, false, false)",
    )
    .bind(id)
    .bind(&body.title)
    .bind(&slug)
    .bind(&body.content)
    .bind(&body.summary)
    .bind(&body.article_type)
    .bind(sop_code)
    .bind(sop_steps)
    .bind(&body.sop_trigger)
    .bind(&body.sop_owner_role)
    .bind(&body.tags)
    .bind(&body.access_level)
    .bind(body.is_private)
    .bind(category_id)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "title": body.title, "status": "draft"}),
    ))
}

/// `_find_article(db, id_or_slug)`: by id, or by slug when the value is not a UUID.
async fn _find_article(db: &mut PgConnection, id_or_slug: &str) -> ApiResult<Option<KBArticle>> {
    let mut article: Option<KBArticle> = None;
    if let Some(id) = granary_compat::text::utils::py_uuid(id_or_slug) {
        article = sqlx::query_as(
            "SELECT * FROM kb_articles WHERE kb_articles.is_deleted = false AND kb_articles.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    }
    if article.is_none() {
        article = sqlx::query_as(
            "SELECT * FROM kb_articles WHERE kb_articles.is_deleted = false AND kb_articles.slug = $1",
        )
        .bind(id_or_slug)
        .fetch_optional(&mut *db)
        .await?;
    }
    Ok(article)
}

async fn user_name(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_scalar("SELECT users.name FROM users WHERE users.id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `GET "/articles/{article_id}"`: by id or slug.
pub async fn get_article(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    _require_kb_view(&user)?;
    let mut conn = db.conn().await?;
    let article = _find_article(&mut conn, &article_id).await?;
    let article = match article {
        Some(a) if _can_read_article(&user, &a) => a,
        _ => return Err(ApiError::not_found("Article", Some(&article_id))),
    };
    let open_revision = _open_revision(&mut conn, &article).await?;
    let created_by_name = user_name(&mut conn, article.created_by_id).await?;
    let owner_signed_by_name = user_name(&mut conn, article.owner_signed_by_id).await?;
    let mut out = json!({
        "id": article.id.to_string(), "title": article.title, "slug": article.slug,
        // SOP revisions: a draft revision of a published SOP, and the published SOP's open revision
        "revision_of": uuid_opt(article.previous_version_id),
        "pending_revision_id": uuid_opt(open_revision.map(|r| r.id)),
        "article_type": article.article_type, "content": article.content,
        "summary": article.summary, "sop_code": article.sop_code,
        "sop_steps": article.sop_steps, "sop_trigger": article.sop_trigger,
        "sop_owner_role": article.sop_owner_role,
        "tags": article.tags, "access_level": article.access_level,
        "is_private": article.is_private,
        "created_by_id": uuid_opt(article.created_by_id),
        "created_by_name": created_by_name,
        "status": article.status, "version": article.version,
        "category_id": uuid_opt(article.category_id),
        "updated_at": resp::iso(&article.updated_at),
        "created_at": resp::iso(&article.created_at),
        // Signature fields
        "author_signed_at": resp::iso_opt(article.author_signed_at.as_ref()),
        "author_signature_url": article.author_signature_url,
        "owner_signed_by_id": uuid_opt(article.owner_signed_by_id),
        "owner_signed_by_name": owner_signed_by_name,
        "owner_signed_at": resp::iso_opt(article.owner_signed_at.as_ref()),
        "owner_signature_url": article.owner_signature_url,
    });
    // PDF import: conversion progress / failure and the source file
    for (k, v) in _pdf_import_dict(&mut conn, &article).await? {
        out[k] = v;
    }
    Ok(resp::json(out))
}

/// `_check_private_access(user, article)`: private articles only for their creator (or a superadmin).
fn _check_private_access(user: &AuthUser, article: &KBArticle) -> ApiResult<()> {
    if article.is_private && !user.is_superadmin && article.created_by_id != Some(user.id) {
        return Err(ApiError::http(403, "This article is private"));
    }
    Ok(())
}

async fn load_article(conn: &mut PgConnection, article_id: &str) -> ApiResult<KBArticle> {
    let a: Option<KBArticle> = sqlx::query_as(
        "SELECT * FROM kb_articles WHERE kb_articles.id = $1 AND kb_articles.is_deleted = false",
    )
    .bind(pg_uuid(article_id)?)
    .fetch_optional(&mut *conn)
    .await?;
    a.ok_or_else(|| ApiError::not_found("Article", Some(article_id)))
}

async fn reload(conn: &mut PgConnection, id: Uuid) -> ApiResult<KBArticle> {
    Ok(
        sqlx::query_as("SELECT * FROM kb_articles WHERE kb_articles.id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// `str(version)` ("None" for a NULL version).
fn version_str(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "None".into())
}

/// The minor-version bump of an in-place edit.
fn bump_minor(version: &Option<String>) -> String {
    let v = version_str(version);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() == 2 {
        if let Some(minor) = py_int(parts[1]) {
            return format!("{}.{}", parts[0], minor + 1);
        }
    }
    format!("{v}.1")
}

/// `PATCH "/articles/{article_id}"`: an edit. A published SOP is never edited in place: the edit opens a
/// new draft revision.
pub async fn update_article(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(mut data): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    if !has_perm(&user, "kb:create") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: kb:create"));
    }
    let mut conn = db.conn().await?;
    let mut article = load_article(&mut conn, &article_id).await?;
    _check_private_access(&user, &article)?;
    _check_sop_permission(&user, article.article_type.as_deref())?;
    if let Some(code) = data.get("sop_code").cloned() {
        let s = if truthy(&code) {
            py_str(&code)
        } else {
            String::new()
        };
        let s = pystr::strip(&s).to_string();
        data.insert(
            "sop_code".into(),
            if s.is_empty() {
                PyObj::None
            } else {
                PyObj::Str(s)
            },
        );
    }
    let change_summary = data
        .shift_remove("change_summary")
        .unwrap_or_else(|| PyObj::Str("Updated".into()));

    let mut created_revision = false;
    let mut sets: Vec<(&str, Bound)> = Vec::new();
    if article.article_type.as_deref() == Some("sop")
        && article.status.as_deref() == Some("published")
    {
        // A published SOP is never edited in place: the edit opens a new draft revision that goes
        // through signing and approval, and the published version stays live meanwhile.
        if let Some(open) = _open_revision(&mut conn, &article).await? {
            return Err(ApiError::http_json(
                409,
                json!({
                    "code": "revision_open", "revision_id": open.id.to_string(),
                    "message": "This SOP already has a draft revision: edit that revision instead",
                }),
            ));
        }
        article = _create_revision(&mut conn, &article, &user).await?;
        created_revision = true;
        data.shift_remove("sop_code"); // the code stays on the published SOP (it is unique)
    } else {
        if article.previous_version_id.is_some() {
            data.shift_remove("sop_code");
        }
        // Save revision before updating
        let summary = bind_val(Kind::Text, &Val::Obj(change_summary.clone()))?;
        let Bound::Text(summary) = summary else {
            unreachable!()
        };
        sqlx::query(
            "INSERT INTO kb_article_revisions (id, article_id, version, content_snapshot, changed_by_id, \
             change_summary) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(article.id)
        .bind(version_str(&article.version))
        .bind(&article.content)
        .bind(user.id)
        .bind(summary)
        .execute(&mut *conn)
        .await?;
        // Bump minor version
        let version = bump_minor(&article.version);
        article.version = Some(version.clone());
        sets.push(("version", Bound::Text(Some(version))));
        if article.article_type.as_deref() == Some("sop")
            && (article.author_signed_at.is_some()
                || article.owner_signed_at.is_some()
                || article.status.as_deref() == Some("review"))
        {
            // Signatures and any approval were given for the old text
            for field in SOP_SIGNATURE_FIELDS {
                sets.push((
                    field,
                    match *field {
                        "author_signed_at" | "owner_signed_at" => Bound::Ts(None),
                        "owner_signed_by_id" => Bound::Uuid(None),
                        _ => Bound::Text(None),
                    },
                ));
            }
            sets.push(("status", Bound::Text(Some("draft".into()))));
            article.status = Some("draft".into());
            engine::supersede_for_entity(
                &mut conn,
                "sop_article",
                article.id,
                "Edited after signing or submission",
            )
            .await?;
        }
    }
    if created_revision {
        sets.push((
            "version_notes",
            bind_val(Kind::Text, &Val::Obj(change_summary))?,
        ));
    }
    for (k, v) in &data {
        let (col, kind): (&'static str, Kind) = match k.as_str() {
            "title" => ("title", Kind::Text),
            "content" => ("content", Kind::Text),
            "summary" => ("summary", Kind::Text),
            "tags" => ("tags", Kind::TextArr),
            "access_level" => ("access_level", Kind::Text),
            "is_private" => ("is_private", Kind::Bool),
            "sop_steps" => ("sop_steps", Kind::Json),
            "sop_code" => ("sop_code", Kind::Text),
            "sop_trigger" => ("sop_trigger", Kind::Text),
            "sop_owner_role" => ("sop_owner_role", Kind::Text),
            "category_id" => ("category_id", Kind::Uuid),
            _ => continue,
        };
        sets.push((col, bind_val(kind, &Val::Obj(v.clone()))?));
    }
    update_row(&mut conn, "kb_articles", article.id, &sets).await?;
    let article = reload(&mut conn, article.id).await?;
    drop(conn);
    db.commit().await?;
    _queue_reindex(article.id).await;
    Ok(resp::json(json!({
        "id": article.id.to_string(), "version": article.version, "status": article.status,
        "created_revision": created_revision,
        "revision_of": uuid_opt(article.previous_version_id),
    })))
}

/// `PATCH "/articles/{article_id}/publish"`.
pub async fn publish_article(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    if !has_perm(&user, "kb:publish") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: kb:publish"));
    }
    let mut conn = db.conn().await?;
    let article = load_article(&mut conn, &article_id).await?;
    _check_private_access(&user, &article)?;
    _check_sop_permission(&user, article.article_type.as_deref())?;
    let is_sop = article.article_type.as_deref() == Some("sop");
    if is_sop {
        if article.author_signed_at.is_none() || article.owner_signed_at.is_none() {
            return Err(ApiError::http(
                400,
                "SOP must be signed by its author and the Owner before it can be published",
            ));
        }
    } else if article.author_signed_at.is_none() {
        return Err(ApiError::http(
            400,
            "Article must be signed by its author before it can be published",
        ));
    }
    let target = if is_sop {
        let entity = Entity::SopArticle(Box::new(article.clone()));
        engine::ensure_or_submit(&mut conn, "sop_article", &entity, &user, "published", None)
            .await?;
        let mut article = reload(&mut conn, article.id).await?;
        let status = article.status.as_deref();
        if !matches!(status, Some("published") | Some("archived")) {
            // approval may already have published it
            publish_sop_article(&mut conn, &mut article, Some(&user)).await?
        } else if status == Some("archived") && article.previous_version_id.is_some() {
            reload(&mut conn, article.previous_version_id.unwrap()).await?
        } else {
            article
        }
    } else {
        update_row(
            &mut conn,
            "kb_articles",
            article.id,
            &[
                ("status", Bound::Text(Some("published".into()))),
                ("published_by_id", Bound::Uuid(Some(user.id))),
            ],
        )
        .await?;
        reload(&mut conn, article.id).await?
    };
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    _queue_reindex(target.id).await;
    Ok(resp::json(
        json!({"id": target.id.to_string(), "status": target.status}),
    ))
}

/// `DELETE "/articles/{article_id}"` (204): soft delete.
pub async fn delete_article(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    if !has_perm(&user, "kb:create") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: kb:create"));
    }
    let mut conn = db.conn().await?;
    let article = load_article(&mut conn, &article_id).await?;
    _check_private_access(&user, &article)?;
    _check_sop_permission(&user, article.article_type.as_deref())?;
    update_row(
        &mut conn,
        "kb_articles",
        article.id,
        &[
            ("is_deleted", Bound::Bool(Some(true))),
            ("deleted_at", Bound::Ts(Some(now_utc()))),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;
    _queue_reindex(article.id).await;
    Ok(resp::no_content())
}

/// `GET "/articles/{article_id}/revisions"`.
pub async fn get_article_revisions(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    _require_kb_view(&user)?;
    let mut conn = db.conn().await?;
    let article: Option<KBArticle> = sqlx::query_as(
        "SELECT * FROM kb_articles WHERE kb_articles.id = $1 AND kb_articles.is_deleted = false",
    )
    .bind(pg_uuid(&article_id)?)
    .fetch_optional(&mut *conn)
    .await?;
    let article = match article {
        Some(a) if _can_read_article(&user, &a) => a,
        _ => return Err(ApiError::not_found("Article", Some(&article_id))),
    };
    let revs: Vec<KBArticleRevision> = sqlx::query_as(
        "SELECT * FROM kb_article_revisions WHERE kb_article_revisions.article_id = $1 \
         ORDER BY kb_article_revisions.created_at DESC",
    )
    .bind(article.id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = revs
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(),
                "version": r.version,
                "change_summary": r.change_summary,
                "changed_by_id": uuid_opt(r.changed_by_id),
                "created_at": resp::iso(&r.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

// ─── Signing ─────────────────────────────────────────────────────────────────

/// `_commit_and_dispatch(db)`.
async fn _commit_and_dispatch(db: &Db) -> ApiResult<()> {
    if let Err(e) = db.commit().await {
        notify::discard_outbox();
        return Err(e);
    }
    notify::dispatch_outbox();
    Ok(())
}

/// `_submit_signed_sop(db, article, user)`: both signatures are on the SOP: route it through the
/// "sop_article" approval flow instead of publishing it. With no active policy there is no gate and it
/// publishes now. An Owner who is the approver approves on the spot. Returns the row that ends up published
/// (the live SOP, when this is a revision), else the article itself. `article` is already flushed.
async fn _submit_signed_sop(
    conn: &mut PgConnection,
    article: &KBArticle,
    user: &AuthUser,
) -> ApiResult<KBArticle> {
    let entity = Entity::SopArticle(Box::new(article.clone()));
    if engine::get_policy(conn, "sop_article").await?.is_none()
        || engine::is_approved(conn, "sop_article", &entity).await?
    {
        let mut a = article.clone();
        return publish_sop_article(conn, &mut a, Some(user)).await;
    }
    if let Some(latest) = engine::latest_request(conn, "sop_article", article.id).await? {
        if latest.status == "pending" {
            return reload(conn, article.id).await;
        }
    }
    let request = engine::submit(
        conn,
        "sop_article",
        &article.id.to_string(),
        user,
        None,
        None,
        None,
    )
    .await?;
    let me = Value::String(user.id.to_string());
    let candidate = engine::current_step(&request)
        .and_then(|s| s.get("candidate_user_ids"))
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.contains(&me));
    if request.status == "pending" && user.is_superadmin && candidate {
        engine::act(
            conn,
            &request.id.to_string(),
            user,
            "approved",
            Some("Approved on countersignature"),
            None,
            None,
        )
        .await?;
    }
    let article = reload(conn, article.id).await?;
    if article.status.as_deref() == Some("archived") {
        if let Some(prev) = article.previous_version_id {
            return reload(conn, prev).await;
        }
    }
    Ok(article)
}

/// `signature_url` from an untyped body when truthy.
fn signature_url(body: &IndexMap<String, PyObj>) -> ApiResult<Option<Bound>> {
    match body.get("signature_url") {
        Some(v) if truthy(v) => Ok(Some(bind_val(Kind::Text, &Val::Obj(v.clone()))?)),
        _ => Ok(None),
    }
}

/// `POST "/articles/{article_id}/sign"`: the author signs. Non-SOP articles publish; SOPs also need the
/// Owner's signature.
pub async fn sign_article(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let article = load_article(&mut conn, &article_id).await?;
    if article.created_by_id.is_some() && article.created_by_id != Some(user.id) {
        return Err(ApiError::http(403, "Only the article's author can sign it"));
    }
    if article.author_signed_at.is_some() {
        return Err(ApiError::http(
            400,
            "Article has already been signed by the author",
        ));
    }
    let now = now_utc();
    let mut sets = vec![("author_signed_at", Bound::Ts(Some(now)))];
    if let Some(url) = signature_url(&body)? {
        sets.push(("author_signature_url", url));
    }
    // Auto-publish non-SOP articles when the author signs
    let is_sop = article.article_type.as_deref() == Some("sop");
    if !is_sop {
        sets.push(("status", Bound::Text(Some("published".into()))));
        sets.push(("published_by_id", Bound::Uuid(Some(user.id))));
    }
    update_row(&mut conn, "kb_articles", article.id, &sets).await?;
    let mut target = reload(&mut conn, article.id).await?;
    if is_sop && article.owner_signed_at.is_some() {
        // SOP: the Owner already signed: both signatures are on it, so it goes to approval
        target = _submit_signed_sop(&mut conn, &target, &user).await?;
    }
    let article = reload(&mut conn, article.id).await?;
    drop(conn);
    _commit_and_dispatch(&db).await?;
    let published_id = (target.status.as_deref() == Some("published")).then_some(target.id);
    Ok(resp::json(json!({
        "id": article.id.to_string(),
        "author_signed_at": resp::iso(&now),
        "status": article.status,
        "published_id": uuid_opt(published_id),
    })))
}

/// `POST "/articles/{article_id}/sign/owner"`: the Owner countersigns an SOP; with both signatures it goes
/// to the SOP approval flow (it publishes once approved).
pub async fn sign_article_owner(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    if !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "Only the superadmin (Owner) can countersign SOPs",
        ));
    }
    let mut conn = db.conn().await?;
    let article = load_article(&mut conn, &article_id).await?;
    if article.article_type.as_deref() != Some("sop") {
        return Err(ApiError::http(
            400,
            "Owner countersignature is only required for SOPs",
        ));
    }
    if article.owner_signed_at.is_some() {
        return Err(ApiError::http(
            400,
            "This SOP has already been countersigned by the Owner",
        ));
    }
    let now = now_utc();
    let mut sets = vec![
        ("owner_signed_by_id", Bound::Uuid(Some(user.id))),
        ("owner_signed_at", Bound::Ts(Some(now))),
    ];
    if let Some(url) = signature_url(&body)? {
        sets.push(("owner_signature_url", url));
    }
    update_row(&mut conn, "kb_articles", article.id, &sets).await?;
    let mut target = reload(&mut conn, article.id).await?;
    if article.author_signed_at.is_some() {
        // Both signatures are on it: submit to the SOP approval flow (it publishes once approved)
        target = _submit_signed_sop(&mut conn, &target, &user).await?;
    }
    let article = reload(&mut conn, article.id).await?;
    drop(conn);
    _commit_and_dispatch(&db).await?;
    let published_id = (target.status.as_deref() == Some("published")).then_some(target.id);
    Ok(resp::json(json!({
        "id": article.id.to_string(),
        "owner_signed_at": resp::iso(&now),
        "status": article.status,
        "published_id": uuid_opt(published_id),
    })))
}

// ─── PDF Export ───────────────────────────────────────────────────────────────

/// `urllib.parse.quote(s, safe='')`.
fn url_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// routers/signing.py `_content_disposition(disposition, filename)`: an ASCII fallback plus the exact
/// UTF-8 name (RFC 6266 / 5987).
fn _content_disposition(disposition: &str, filename: &str) -> String {
    let fallback: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii() && (c.is_ascii_alphanumeric() || " -_.".contains(c)) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fallback = pystr::strip(&fallback);
    let fallback = if fallback.is_empty() {
        "document"
    } else {
        fallback
    };
    format!(
        "{disposition}; filename=\"{fallback}\"; filename*=UTF-8''{}",
        url_quote(filename)
    )
}

/// `GET "/articles/{article_id}/pdf"`: the article/SOP as a PDF with letterhead and signature blocks.
pub async fn export_article_pdf(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    _require_kb_view(&user)?;
    let mut conn = db.conn().await?;
    let article = match _find_article(&mut conn, &article_id).await? {
        Some(a) if _can_read_article(&user, &a) => a,
        _ => return Err(ApiError::not_found("Article", Some(&article_id))),
    };
    let data =
        granary_services::verification::render::sop_template_data(&mut conn, &article).await?;
    drop(conn);
    let pdf_bytes = granary_services::pdf::generate_pdf("sop", &data, None, None).await?;
    let name = match article.sop_code.as_deref() {
        Some(c) if !c.is_empty() => c,
        _ => &article.slug,
    };
    let filename = format!("{name}.pdf");
    Ok(resp::with_headers(
        resp::bytes_response(200, pdf_bytes, "application/pdf"),
        &[(
            "content-disposition",
            _content_disposition("attachment", &filename),
        )],
    ))
}

/// Python `\s` (str patterns).
fn re_space(c: char) -> bool {
    pystr::is_space(c)
}

/// `re.match(r"^#+\s+(.+)", body, re.MULTILINE)`: group 1, or None.
fn heading_match(body: &str) -> Option<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() && chars[i] == '#' {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let ws_start = i;
    while i < chars.len() && re_space(chars[i]) {
        i += 1;
    }
    let ws_end = i;
    if ws_end == ws_start {
        return None;
    }
    // .+ greedy from j: up to the next "\n"
    let take_line =
        |j: usize| -> String { chars[j..].iter().take_while(|c| **c != '\n').collect() };
    if ws_end < chars.len() {
        // chars[ws_end] is not whitespace, so not "\n": .+ matches from here
        return Some(take_line(ws_end));
    }
    // \s+ ran to the end: backtrack it, longest first, until .+ can start on a non-newline
    let mut j = ws_end - 1;
    while j > ws_start {
        if chars[j] != '\n' {
            return Some(take_line(j));
        }
        j -= 1;
    }
    None
}

/// `POST "/articles/import-markdown"` (201): a .md file as a draft article (YAML frontmatter supported).
pub async fn import_markdown(
    InternalUser(user): InternalUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.expect("validated");
    if !has_perm(&user, "kb:create") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: kb:create"));
    }
    if file.filename.is_empty() || !file.filename.ends_with(".md") {
        return Err(ApiError::http(422, "Only .md files are accepted"));
    }
    let file_content = String::from_utf8_lossy(&file.data).into_owned();
    let (meta, body) = _parse_frontmatter(&file_content);

    let stem = file.filename.replace(".md", "");
    let mut title = match meta.get("title") {
        None => String::new(),
        Some(Meta::Str(s)) => pystr::strip(s).to_string(),
        Some(Meta::List(l)) if l.is_empty() => String::new(),
        // (meta.get("title") or "").strip() on a list: AttributeError
        Some(Meta::List(_)) => {
            return Err(super::projects::db_500(
                "AttributeError: 'list' object has no attribute 'strip'",
            ))
        }
    };
    if title.is_empty() {
        title = match heading_match(&body) {
            Some(g) => pystr::strip(&g).to_string(),
            None => stem.clone(),
        };
    }
    let mut base_slug = _slugify(&title);
    if base_slug.is_empty() {
        base_slug = _slugify(&stem);
    }
    let mut conn = db.conn().await?;
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT kb_articles.id FROM kb_articles WHERE kb_articles.slug = $1 AND kb_articles.is_deleted = false",
    )
    .bind(&base_slug)
    .fetch_optional(&mut *conn)
    .await?;
    if existing.is_some() {
        base_slug = format!("{base_slug}-{}", chrono::Utc::now().timestamp());
    }
    let tags: Vec<String> = match meta.get("tags") {
        None => Vec::new(),
        Some(Meta::List(l)) => l.clone(),
        Some(Meta::Str(s)) => s
            .split(',')
            .map(|t| pystr::strip(t).to_string())
            .filter(|t| !t.is_empty())
            .collect(),
    };
    // Text columns take a str; a frontmatter list there is an asyncpg error (500)
    let text = |v: Option<&Meta>, default: Option<&str>| -> ApiResult<Option<String>> {
        match v {
            None => Ok(default.map(str::to_string)),
            Some(Meta::Str(s)) => Ok(Some(s.clone())),
            Some(Meta::List(_)) => Err(super::projects::db_500(
                "invalid input for query argument: a list for a text column",
            )),
        }
    };
    // meta.get("summary") or None: an empty value is None
    let summary = match meta.get("summary") {
        Some(Meta::Str(s)) if s.is_empty() => None,
        Some(Meta::List(l)) if l.is_empty() => None,
        other => text(other, None)?,
    };
    let article_type = text(meta.get("type"), Some("wiki"))?;
    let access_level = text(meta.get("access_level"), Some("all_staff"))?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO kb_articles (id, title, slug, content, summary, article_type, access_level, tags, status, \
         version, created_by_id, sop_steps, is_private, related_article_ids, view_count, helpful_votes, \
         unhelpful_votes, pdf_import_truncated, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'draft', '1.0', $9, '[]', false, '{}', 0, 0, 0, false, false)",
    )
    .bind(id)
    .bind(&title)
    .bind(&base_slug)
    .bind(&body)
    .bind(summary)
    .bind(article_type)
    .bind(access_level)
    .bind(&tags)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "title": title, "slug": base_slug, "status": "draft"}),
    ))
}

granary_core::py_model! {
    /// `class ImportPdfBody(BaseModel)`.
    pub struct ImportPdfBody {
        /// Defaults to the article's existing source PDF, so this doubles as "re-convert".
        pub attachment_id: Option<String> = None,
        /// Conversion overwrites the body, so it only runs on an empty article unless forced.
        pub force: bool = false,
    }
}

/// `POST "/articles/{article_id}/import-pdf"` (202): queue a PDF to Markdown conversion for this article.
pub async fn import_pdf(
    Path(article_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<ImportPdfBody>,
) -> ApiResult<Response> {
    if !has_perm(&user, "kb:create") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: kb:create"));
    }
    let mut conn = db.conn().await?;
    let article = load_article(&mut conn, &article_id).await?;
    _check_sop_permission(&user, article.article_type.as_deref())?;
    let att_id = match body.attachment_id.as_deref() {
        Some(a) if !a.is_empty() => Some(a.to_string()),
        _ => article.source_pdf_attachment_id.map(|u| u.to_string()),
    };
    let Some(att_id) = att_id else {
        return Err(ApiError::http(400, "No PDF attached to this article"));
    };
    let att: Option<Attachment> =
        sqlx::query_as("SELECT * FROM attachments WHERE attachments.id = $1")
            .bind(pg_uuid(&att_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let att = match att {
        Some(a) if a.entity_type == "kb_article" && a.entity_id == article.id.to_string() => a,
        _ => {
            return Err(ApiError::http(
                404,
                "PDF attachment not found on this article",
            ))
        }
    };
    if att.mime_type != "application/pdf" {
        return Err(ApiError::http(422, "Only PDF files can be converted"));
    }
    if !pystr::strip(article.content.as_deref().unwrap_or("")).is_empty() && !body.force {
        return Err(ApiError::http(
            409,
            "This article already has content. Re-run with force to replace it.",
        ));
    }
    update_row(
        &mut conn,
        "kb_articles",
        article.id,
        &[
            ("source_pdf_attachment_id", Bound::Uuid(Some(att.id))),
            ("pdf_import_status", Bound::Text(Some("pending".into()))),
            ("pdf_import_error", Bound::Text(None)),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;

    let mut status = "pending".to_string();
    let mut error: Option<String> = None;
    if let Some(queue_error) = _queue_pdf_import(article.id).await {
        let mut conn = db.conn().await?;
        update_row(
            &mut conn,
            "kb_articles",
            article.id,
            &[
                ("pdf_import_status", Bound::Text(Some("failed".into()))),
                ("pdf_import_error", Bound::Text(Some(queue_error.clone()))),
            ],
        )
        .await?;
        drop(conn);
        db.commit().await?;
        status = "failed".into();
        error = Some(queue_error);
    }
    Ok(resp::json_status(
        202,
        json!({"id": article.id.to_string(), "pdf_import_status": status,
               "pdf_import_error": error,
               "source_pdf": {"id": att.id.to_string(), "file_name": att.file_name}}),
    ))
}
