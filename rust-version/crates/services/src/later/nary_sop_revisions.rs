//! routers/knowledge.py `_open_revision` / `_create_revision`, which services/nary/sop_revisions.py calls.
//! Ported here (wave 4, group 15) so both the router (crates/api/src/routers/knowledge.rs) and the services
//! crate use one implementation (the services crate cannot depend on the api crate).

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use granary_db::rows::knowledge::KBArticle;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

/// routers/knowledge.py `_open_revision(db, article)`: the draft (or in-review) revision of a published SOP.
pub async fn _open_revision(
    db: &mut PgConnection,
    article: &KBArticle,
) -> ApiResult<Option<KBArticle>> {
    Ok(sqlx::query_as(
        "SELECT * FROM kb_articles WHERE kb_articles.previous_version_id = $1 \
         AND kb_articles.is_deleted = false AND kb_articles.status IN ('draft', 'review') \
         ORDER BY kb_articles.created_at DESC LIMIT 1",
    )
    .bind(article.id)
    .fetch_optional(&mut *db)
    .await?)
}

/// Python `int(s)` for a str: whitespace, a sign, decimal digits (any script) with single underscores.
fn py_int(s: &str) -> Option<i64> {
    let t = granary_compat::text::pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = granary_compat::text::pystr::decimal_value(c)?;
        n = n.checked_mul(10)?.checked_add(d as i64)?;
    }
    Some(if neg { -n } else { n })
}

/// `_bump_major(version)`: `f"{int(str(version).split('.')[0]) + 1}.0"`, else `f"{version}.1"`.
pub fn _bump_major(version: Option<&str>) -> String {
    let v = version.unwrap_or("None");
    match py_int(v.split('.').next().unwrap_or("")) {
        Some(n) => format!("{}.0", n + 1),
        None => format!("{v}.1"),
    }
}

/// routers/knowledge.py `_create_revision(db, article, user)`: a new draft revision of a published SOP
/// (flushed). The published row stays live until the revision is signed and approved
/// (adapters.publish_sop_article); the revision's editor is its author.
pub async fn _create_revision(
    db: &mut PgConnection,
    article: &KBArticle,
    user: &AuthUser,
) -> ApiResult<KBArticle> {
    let base = format!("{}-rev", article.slug);
    let mut slug = base.clone();
    let mut n = 1;
    loop {
        let taken: Option<(Uuid,)> =
            sqlx::query_as("SELECT kb_articles.id FROM kb_articles WHERE kb_articles.slug = $1")
                .bind(&slug)
                .fetch_optional(&mut *db)
                .await?;
        if taken.is_none() {
            break;
        }
        n += 1;
        slug = format!("{base}-{n}");
    }
    // KBArticle(slug=, article_type=, sop_code=None, status="draft", version=, previous_version_id=,
    // created_by_id=, **SOP_CONTENT_FIELDS); the other columns take their Python defaults. sop_steps set
    // to None is stored as the JSON null (JSON none_as_null=False).
    Ok(sqlx::query_as(
        "INSERT INTO kb_articles (id, slug, article_type, sop_code, status, version, previous_version_id, \
         created_by_id, title, content, summary, sop_steps, sop_trigger, sop_owner_role, tags, access_level, \
         is_private, category_id, related_article_ids, view_count, helpful_votes, unhelpful_votes, \
         pdf_import_truncated, is_deleted) \
         VALUES ($1, $2, $3, NULL, 'draft', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
         '{}', 0, 0, 0, false, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&slug)
    .bind(&article.article_type)
    .bind(_bump_major(article.version.as_deref()))
    .bind(article.id)
    .bind(user.id)
    .bind(&article.title)
    .bind(&article.content)
    .bind(&article.summary)
    .bind(article.sop_steps.clone().unwrap_or(Value::Null))
    .bind(&article.sop_trigger)
    .bind(&article.sop_owner_role)
    .bind(&article.tags)
    .bind(&article.access_level)
    .bind(article.is_private)
    .bind(article.category_id)
    .fetch_one(&mut *db)
    .await?)
}
